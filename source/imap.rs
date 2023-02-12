// I don't feel like nom is very suitable (from a cursory glance at the code).
// LALRPOP and Pest don't support bytes (https://github.com/lalrpop/lalrpop/issues/230,
// https://github.com/pest-parser/pest/issues/244).

use anyhow::Context as _;
use base64::Engine as _;
use std::{borrow, cell, cmp, io, str};

// Inclusive.
#[derive(Debug, PartialEq)]
pub struct Range(pub u64, pub u64);

#[derive(Debug, PartialEq)]
pub enum Mailbox<'input> {
  Inbox,
  Other(borrow::Cow<'input, [u8]>),
}

#[derive(Debug, PartialEq)]
pub struct SelectFetch<'input> {
  pub uid: u64,
  pub flags: Vec<&'input [u8]>,
  pub modseq: u64,
}

#[derive(Debug, PartialEq)]
pub enum Select<'input> {
  Flags(Vec<&'input [u8]>),
  UIDValidity(u64),
  HighestModSeq(u64),
  Vanished(Vec<Range>),
  Fetch(SelectFetch<'input>),
}

#[derive(Debug, PartialEq)]
pub struct Append {
  pub uidvalidity: u64,
  pub uid: u64,
}

#[derive(Debug, PartialEq)]
pub struct Store {
  pub uid: u64,
  pub modseq: u64,
}

#[derive(Debug, PartialEq)]
pub struct Move {
  pub uidvalidity: u64,
  pub from: Vec<Range>,
  pub to: Vec<Range>,
}

fn parse_number(n: &[u8]) -> u64 {
  // One unwrap could be eliminiated since it's guaranteed by the BNF but it's either that or
  // unsafe...
  str::from_utf8(n).unwrap().parse().unwrap()
}

// The naive l:$(CHAR8()*<{n}>) in literal() would result in pushing every CHAR8() into the vector
// before discarding it because we reference it: https://github.com/kevinmehall/rust-peg/pull/292
// That's probably the most critical part of the parser since that's how emails are transferred.
// Instead, we use an undocumented escape hatch to do a fast skip (CHAR8() excludes null bytes but
// it shouldn't really matter): https://github.com/kevinmehall/rust-peg/issues/284
trait ParserHacks {
  fn skip(&self, position: usize, n: usize) -> peg::RuleResult<()>;
}

impl ParserHacks for [u8] {
  fn skip(&self, position: usize, n: usize) -> peg::RuleResult<()> {
    if self.len() >= position + n {
      return peg::RuleResult::Matched(position + n, ());
    }
    peg::RuleResult::Failed
  }
}

peg::parser! {
  // https://www.rfc-editor.org/rfc/rfc2234#section-2.3
  // https://www.rfc-editor.org/rfc/rfc3501#section-9
  pub grammar parser() for [u8] {
    // CR = %x0D
    rule CR() = "\r"
    // LF = %x0A
    rule LF() = "\n"
    // CRLF = CR LF
    rule CRLF() = CR() LF()
    // CHAR = %x01-7F
    rule CHAR() -> u8
      = [b'\x01'..=b'\x7f']
    // CHAR8 = %x01-ff
    rule CHAR8() = [b'\x01'..=b'\xff']
    // CTL = %x00-1F / %x7F
    rule CTL() = [b'\x00'..=b'\x1f'] / "\x7f"
    // DQUOTE = %x22
    rule DQUOTE() -> u8
      = "\""
      { b'"' }
    // In all cases, SP refers to exactly one space. It is NOT permitted to substitute TAB, insert
    // additional spaces, or otherwise treat SP as being equivalent to LWSP.
    rule SP() = " "
    // TEXT-CHAR = <any CHAR except CR and LF>
    rule TEXT_CHAR() -> u8
      = !(CR() / LF()) c:CHAR()
      { c }
    // DIGIT = %x30-39
    rule DIGIT() = [b'\x30'..=b'\x39']
    // digit-nz = %x31-39
    rule digit_nz() = [b'\x31'..=b'\x39']

    // number = 1*DIGIT
    rule number() -> u64
      = n:$(DIGIT()+)
      { parse_number(n) }
    // nz-number = digit-nz *DIGIT
    rule nz_number() -> u64
      = n:$(digit_nz() DIGIT()*)
      { parse_number(n) }
    // uniqueid = nz-number
    rule uniqueid() -> u64 = nz_number()
    // text = 1*TEXT-CHAR
    rule text() = TEXT_CHAR()+

    // nil = "NIL"
    rule nil() = "NIL"
    // list-wildcards = "%" / "*"
    rule list_wildcards() = "%" / "*"
    // quoted-specials = DQUOTE / "\"
    rule quoted_specials() -> u8
      = c:(DQUOTE() / ("\\" { b'\\' }))
      { c }
    // QUOTED-CHAR = <any TEXT-CHAR except quoted-specials> / "\" quoted-specials
    rule QUOTED_CHAR() -> u8
      = !quoted_specials() c:TEXT_CHAR() { c } / "\\" c:quoted_specials()
      { c }
    // resp-specials = "]"
    rule resp_specials() = "]"
    // atom-specials = "(" / ")" / "{" / SP / CTL / list-wildcards / quoted-specials / resp-specials
    rule atom_specials() = "(" / ")" / "{" / SP() / CTL() / list_wildcards() / quoted_specials() / resp_specials()
    // ATOM-CHAR = <any CHAR except atom-specials>
    rule ATOM_CHAR() = !atom_specials() CHAR()
    // atom = 1*ATOM-CHAR
    rule atom() = ATOM_CHAR()+
    // ASTRING-CHAR = ATOM-CHAR / resp-specials
    rule ASTRING_CHAR() = ATOM_CHAR() / resp_specials()
    // quoted = DQUOTE *QUOTED-CHAR DQUOTE
    rule quoted() -> Vec<u8>
      = DQUOTE() q:(QUOTED_CHAR()*) DQUOTE()
      { q }
    // literal = "{" number "}" CRLF *CHAR8
    rule literal() -> &'input [u8]
      = "{" n:number() "}" CRLF() position!() l:$(##skip(usize::try_from(n).unwrap() /* not much we can do */))
      { l }
    // string = quoted / literal
    rule string() -> borrow::Cow<'input, [u8]>
      = q:quoted() { borrow::Cow::Owned(q) } / l:literal() { borrow::Cow::Borrowed(l) }
    // astring = 1*ASTRING-CHAR / string
    rule astring() -> borrow::Cow<'input, [u8]>
      = s:$(ASTRING_CHAR()+) { borrow::Cow::Borrowed(s) } / s:string() { s }
    // nstring = string / nil
    rule nstring() -> Option<borrow::Cow<'input, [u8]>>
      = s:string() { Some(s) } / nil() { None }

    // tag = 1*<any ASTRING-CHAR except "+">
    rule tag() -> &'input [u8] = $((!"+" ASTRING_CHAR())+)

    // auth-type = atom
    rule auth_type() = atom()
    // capability = ("AUTH=" auth-type) / atom
    rule capability() -> &'input [u8] = $(("AUTH=" auth_type()) / atom())
    // capability-data = "CAPABILITY" *(SP capability) SP "IMAP4rev1" *(SP capability)
    // Rewritten for simplicity and to avoid backtracking (capability can match "IMAP4rev1").
    rule capability_data() -> Vec<&'input [u8]>
      = "CAPABILITY" cs:(SP() c:capability() { c })+
      { cs }

    // mailbox = "INBOX" / astring
    rule mailbox() -> Mailbox<'input>
      = ("i" / "I") ("n" / "N") ("b" / "B") ("o" / "O") ("x" / "X") { Mailbox::Inbox } / m:astring() { Mailbox::Other(m) }
    // flag-extension = "\" atom
    // mbx-list-oflag = "\Noinferiors" / flag-extension
    // mbx-list-sflag = "\Noselect" / "\Marked" / "\Unmarked"
    // mbx-list-flags = *(mbx-list-oflag SP) mbx-list-sflag *(SP mbx-list-oflag) / mbx-list-oflag *(SP mbx-list-oflag)
    // Rewritten for simplicity.
    rule mbx_list_flags() -> Vec<&'input [u8]>
      = fs:((f:$("\\" atom()) { f }) ** SP())
      { fs }
    // mailbox-list = "(" [mbx-list-flags] ")" SP (DQUOTE QUOTED-CHAR DQUOTE / nil) SP mailbox
    rule mailbox_list() -> (Vec<&'input [u8]>, Option<u8>, Mailbox<'input>)
      = "(" fs:mbx_list_flags() ")" SP() c:(DQUOTE() c:QUOTED_CHAR() DQUOTE() { Some(c) } / nil() { None }) SP() m:mailbox()
      { (fs, c, m) }

    // flag-keyword = atom
    rule flag_keyword() -> &'input [u8] = $(atom())
    // flag-extension = "\" atom
    rule flag_extension() -> &'input [u8] = $("\\" atom())
    // flag = "\Answered" / "\Flagged" / "\Deleted" / "\Seen" / "\Draft" / flag-keyword / flag-extension
    // This rule is equivalent because flag-extension allows any of the system flags.
    rule flag() -> &'input [u8] = flag_keyword() / flag_extension()
    // flag-perm = flag / "\*"
    rule flag_perm() -> &'input [u8] = f:flag() { f } / $("\\*")
    // flag-fetch = flag / "\Recent"
    // This rule is equivalent (because flag allows any system flag).
    rule flag_fetch() -> &'input [u8] = flag()

    // mod-sequence-value = 1*DIGIT
    rule mod_sequence_value() -> u64
      = n:$(DIGIT()+)
      { parse_number(n) }
    // https://www.rfc-editor.org/rfc/rfc7162#section-7
    // permsg-modsequence = mod-sequence-value
    rule permsg_modsequence() -> u64 = mod_sequence_value()

    // msg-att-static = ... / "UID" SP uniqueid /...
    rule msg_att_static_uid() -> u64
      = "UID" SP() u:uniqueid()
      { u }
    // msg-att-dynamic = "FLAGS" SP "(" [flag-fetch *(SP flag-fetch)] ")"
    rule msg_att_dynamic_flags() -> Vec<&'input [u8]>
      = "FLAGS" SP() "(" fs:(flag_fetch() ** SP()) ")"
      { fs }
    // https://www.rfc-editor.org/rfc/rfc7162#section-7
    // fetch-mod-resp = "MODSEQ" SP "(" permsg-modsequence ")"
    // msg-att-dynamic =/ fetch-mod-resp
    rule fetch_mod_resp() -> u64
      = "MODSEQ" SP() "(" m:permsg_modsequence() ")"
      { m }

    // seq-number = nz-number / "*"
    rule seq_number() -> Range = n:nz_number() { Range(n, n) } / "*" { Range(0, u64::max_value()) }
    // seq-range = seq-number ":" seq-number
    // Example: 2:4 and 4:2 are equivalent and indicate values 2, 3, and 4.
    rule seq_range() -> Range
      = r1:seq_number() ":" r2:seq_number()
      {
        if r1.0 <= r2.1 {
          Range(r1.0, r2.1)
        } else {
          Range(r2.0, r1.0)
        }
      }
    // sequence-set = (seq-number / seq-range) *("," sequence-set)
    // Rewritten for simplicitly and to avoid backtracking (seq-number can match seq-range).
    rule sequence_set() -> Vec<Range> = (seq_range() / seq_number()) ** ","
    // https://www.rfc-editor.org/rfc/rfc7162#section-7
    // known-uids = sequence-set
    rule known_uids() -> Vec<Range> = sequence_set()
    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // append-uid = uniqueid
    rule append_uid() -> u64 = uniqueid()

    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // uid-range = (uniqueid ":" uniqueid)
    // Example: 2:4 and 4:2 are equivalent.
    rule uid_range() -> Range
      = u1:uniqueid() ":" u2:uniqueid()
      {
        if u1 <= u2 {
          Range(u1, u2)
        } else {
          Range(u2, u1)
        }
      }
    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // uid-set = (uniqueid / uid-range) *("," uid-set)
    rule uid_set() -> Vec<Range>
      = (u:uniqueid() { Range(u, u) } / uid_range()) ** ","

    // resp-text-code = ... / "PERMANENTFLAGS" SP "(" [flag-perm *(SP flag-perm)] ")" / ...
    rule resp_code_permanentflags() -> Vec<&'input [u8]>
      = "PERMANENTFLAGS" SP() "(" fs:(flag_perm() ** SP()) ")"
      { fs }
    // resp-text-code = ... / "UIDVALIDITY" SP nz-number / ...
    rule resp_code_uidvalidity() -> u64
      = "UIDVALIDITY" SP() n:nz_number()
      { n }
    // https://www.rfc-editor.org/rfc/rfc4551#section-3.6
    // resp-text-code =/ "HIGHESTMODSEQ" SP mod-sequence-value / ...
    rule resp_code_highestmodseq() -> u64
      = "HIGHESTMODSEQ" SP() n:mod_sequence_value()
      { n }
    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // resp-code-apnd = "APPENDUID" SP nz-number SP append-uid
    rule resp_code_apnd() -> Append
      = "APPENDUID" SP() n:nz_number() SP() u:append_uid()
      { Append { uidvalidity: n, uid: u } }
    // https://www.rfc-editor.org/rfc/rfc4551#section-4
    // https://www.rfc-editor.org/errata/eid3506
    // resp-text-code =/ ... / "MODIFIED" SP sequence-set
    rule resp_code_modified() -> Vec<Range>
      = "MODIFIED" SP() s:sequence_set()
      { s }
    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // resp-code-copy = "COPYUID" SP nz-number SP uid-set SP uid-set
    rule resp_code_copy() -> Move
      = "COPYUID" SP() n:nz_number() SP() us1:uid_set() SP() us2:uid_set()
      { Move { uidvalidity: n, from:us1, to: us2 } }

    // https://www.rfc-editor.org/rfc/rfc3501#section-2.2.2
    // Data transmitted by the server to the client and status responses that do not indicate
    // command completion are prefixed with the token "*", and are called untagged responses.
    // [...]
    // The server completion result response indicates the success or failure of the operation. It
    // is tagged with the same tag as the client command which began the operation.
    // [...]
    // A client MUST be prepared to accept any server response at all times. This includes server
    // data that was not requested.
    #[no_eof]
    pub rule start() -> (usize, &'input [u8])
      = s:($("*") / tag()) SP() p:position!()
      { (p, s) }

    // TODO? replace text with CHAR8? search for literals?
    #[no_eof]
    pub rule skip() -> (usize, ())
      = text() CRLF() p:position!()
      { (p, ()) }

    // resp-text = ["[" resp-text-code "]" SP] text
    // resp-cond-auth = ("OK" / "PREAUTH") SP resp-text
    // resp-cond-state = ("OK" / "NO" / "BAD") SP resp-text
    #[no_eof]
    pub rule bad() -> (usize, ())
      = "BAD" SP() text() CRLF() p:position!()
      { (p, ()) }
    #[no_eof]
    pub rule ok() -> (usize, ())
      = "OK" SP() text() CRLF() p:position!()
      { (p, ()) }

    // resp-text-code = ... / capability-data / ...
    // resp-text = ["[" resp-text-code "]" SP] text
    // resp-cond-auth = ("OK" / "PREAUTH") SP resp-text
    // greeting = "*" SP (resp-cond-auth / resp-cond-bye) CRLF
    //
    // https://www.rfc-editor.org/rfc/rfc3501#section-6.2.2
    // A server MAY include a CAPABILITY response code in the tagged OK response of a successful
    // AUTHENTICATE command in order to send capabilities automatically.
    //
    // https://www.rfc-editor.org/rfc/rfc3501#section-7.1.4
    // The PREAUTH response is always untagged, and is one of three possible greetings at connection
    // startup. It indicates that the connection has already been authenticated by external means;
    // thus no LOGIN command is needed.
    //
    // We're only concerned about the capabilities in the greetings so inline that and discard the
    // rest. PREAUTH isn't supported.
    #[no_eof]
    pub rule available_capabilities() -> (usize, Vec<&'input [u8]>)
      = "OK" SP() "[" cs:capability_data() "]" SP() text() CRLF() p:position!()
      { (p, cs) }

    // https://www.rfc-editor.org/rfc/rfc5161
    // enable-data = "ENABLED" *(SP capability)
    // response-data =/ "*" SP enable-data CRLF
    #[no_eof]
    pub rule enabled_capabilities() -> (usize, Vec<&'input [u8]>)
      = "ENABLED" cs:((SP() c:capability() { c })*) CRLF() p:position!()
      { (p, cs) }

    // mailbox-data = ... / "LIST" SP mailbox-list / ...
    // response-data = "*" SP (... / mailbox-data / ...) CRLF
    #[no_eof]
    pub rule list_mailbox() -> (usize, (Vec<&'input [u8]>, Option<u8>, Mailbox<'input>))
      = "LIST" SP() l:mailbox_list() CRLF() p:position!()
      { (p, l) }

    // resp-text = ["[" resp-text-code "]" SP] text
    // resp-cond-state = ("OK" / "NO" / "BAD") SP resp-text
    // response-tagged = tag SP resp-cond-state CRLF
    //
    // https://www.rfc-editor.org/rfc/rfc4551#section-3.1.1
    // A server [...] MUST send the OK untagged response including HIGHESTMODSEQ response
    // code with every successful SELECT or EXAMINE command.
    //
    // https://www.rfc-editor.org/rfc/rfc7162#section-3.2.5.1
    // The server sends the client any pending flag changes (using FETCH responses that MUST contain
    // UIDs) [...] that have occurred in this mailbox since the provided modification sequence.
    //
    // msg-att-static = ... / "UID" SP uniqueid /...
    // msg-att = "(" (msg-att-dynamic / msg-att-static) *(SP (msg-att-dynamic / msg-att-static)) ")"
    // message-data = nz-number SP (... / ("FETCH" SP msg-att))
    // response-data = "*" SP (... / message-data / ...) CRLF
    //
    // https://www.rfc-editor.org/rfc/rfc7162#section-3.2.10
    // The VANISHED response has two forms. The first form contains the EARLIER tag, which signifies
    // that the response was caused by a UID FETCH (VANISHED) or a SELECT/EXAMINE (QRESYNC) command.
    // The second form doesn't contain the EARLIER tag and is used for announcing message removals
    // within an already selected mailbox.
    //
    // expunged-resp = "VANISHED" [SP "(EARLIER)"] SP known-uids
    // message-data =/ expunged-resp
    //
    // We're only concerned about the data possibly returned from a SELECT so inline that and
    // discard the rest.
    #[no_eof]
    pub rule select_data() -> (usize, Select<'input>)
      = s:(("OK" SP() "[" s:(
              p:resp_code_permanentflags() { Select::Flags(p) }
            / u:resp_code_uidvalidity() { Select::UIDValidity(u) }
            / h:resp_code_highestmodseq() { Select::HighestModSeq(h) }
            ) "]" SP() text() { s }) /
           ("VANISHED" SP() "(EARLIER)" SP() us:known_uids() { Select::Vanished(us) }) /
           (nz_number() SP() "FETCH" SP() "(" sf:(
              // All possible permutations... I hope I won't have to extend it.
              (u:msg_att_static_uid() SP() fs:msg_att_dynamic_flags() SP() m:fetch_mod_resp() { (u, fs, m) })
            / (u:msg_att_static_uid() SP() m:fetch_mod_resp() SP() fs:msg_att_dynamic_flags() { (u, fs, m) })
            / (fs:msg_att_dynamic_flags() SP() u:msg_att_static_uid() SP() m:fetch_mod_resp() { (u, fs, m) })
            / (fs:msg_att_dynamic_flags() SP() m:fetch_mod_resp() SP() u:msg_att_static_uid() { (u, fs, m) })
            / (m:fetch_mod_resp() SP() u:msg_att_static_uid() SP() fs:msg_att_dynamic_flags() { (u, fs, m) })
            / (m:fetch_mod_resp() SP() fs:msg_att_dynamic_flags() SP() u:msg_att_static_uid() { (u, fs, m) })
           ) ")" { Select::Fetch(SelectFetch { uid: sf.0, flags: sf.1, modseq: sf.2 }) })) CRLF() p:position!()
      { (p, s) }

    // section = "[" [section-spec] "]"
    // msg-att-static = ... / "RFC822.SIZE" SP number / "BODY" section ["<" number ">"] SP nstring / "UID" SP uniqueid /...
    // msg-att = "(" (msg-att-dynamic / msg-att-static) *(SP (msg-att-dynamic / msg-att-static)) ")"
    // message-data = nz-number SP ("EXPUNGE" / ("FETCH" SP msg-att))
    // response-data = "*" SP (... / message-data / ...) CRLF
    //
    // We're only concerned about single body FETCHes so inline that and discard the rest.
    #[no_eof]
    pub rule fetch_size_data() -> (usize, (u64, u64))
      = nz_number() SP() "FETCH" SP() "(" f:(
          (u:msg_att_static_uid() SP() "RFC822.SIZE" SP() n:number() { (u, n) })
        / ("RFC822.SIZE" SP() n:number() SP() u:msg_att_static_uid() { (u, n) })
        ) ")" CRLF() p:position!()
      { (p, f) }
    #[no_eof]
    pub rule fetch_body_data() -> (usize, (u64, Option<borrow::Cow<'input, [u8]>>))
      = nz_number() SP() "FETCH" SP() "(" f:(
          (u:msg_att_static_uid() SP() "BODY[]" SP() s:nstring() { (u, s) })
        / ("BODY[]" SP() s:nstring() SP() u:msg_att_static_uid() { (u, s) })
        ) ")" CRLF() p:position!()
      { (p, f) }

    // resp-text = ["[" resp-text-code "]" SP] text
    // resp-cond-state = ("OK" / "NO" / "BAD") SP resp-text
    // response-tagged = tag SP resp-cond-state CRLF
    //
    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // resp-text-code =/ resp-code-apnd / ...
    #[no_eof]
    pub rule append() -> (usize, Append)
      = "OK" SP() "[" a:resp_code_apnd() "]" SP() text() CRLF() p:position!()
      { (p, a) }

    // I'm not sure which RFC describes this.
    #[no_eof]
    pub rule append_data() -> (usize, u64)
      = "OK" SP() "[" h:resp_code_highestmodseq() "]" SP() text() CRLF() p:position!()
      { (p, h) }

    // resp-text = ["[" resp-text-code "]" SP] text
    // resp-cond-state = ("OK" / "NO" / "BAD") SP resp-text
    // response-tagged = tag SP resp-cond-state CRLF
    //
    // https://www.rfc-editor.org/rfc/rfc4551#section-4
    // https://www.rfc-editor.org/errata/eid3506
    // resp-text-code =/ ... / "MODIFIED" SP sequence-set
    #[no_eof]
    pub rule store() -> (usize, Option<Vec<Range>>)
      = "OK" SP() m:("[" m:resp_code_modified() "]" SP() { m })? text() CRLF() p:position!()
      { (p, m) }

    // https://www.rfc-editor.org/rfc/rfc4551#section-3.2
    // An untagged FETCH response MUST be sent, even if the .SILENT suffix is specified, and the
    // response MUST include the MODSEQ message data item.
    #[no_eof]
    pub rule store_data() -> (usize, Store)
      = nz_number() SP() "FETCH" SP() "(" sf:(
        // All possible permutations... I hope I won't have to extend it.
          (u:msg_att_static_uid() SP() m:fetch_mod_resp() { (u, m) })
        / (m:fetch_mod_resp() SP() u:msg_att_static_uid() { (u, m) })
      ) ")" CRLF() p:position!()
      { (p, Store { uid: sf.0, modseq: sf.1 }) }

    // https://www.rfc-editor.org/rfc/rfc6851#section-3.3
    #[no_eof]
    pub rule move_() /* r#move isn't supported */ -> (usize, Option<u64>)
      = "OK" SP() h:("[" h:resp_code_highestmodseq() "]" SP() { h })? text() CRLF() p:position!()
      { (p, h) }

    // resp-text = ["[" resp-text-code "]" SP] text
    // resp-cond-state = ("OK" / "NO" / "BAD") SP resp-text
    // response-tagged = tag SP resp-cond-state CRLF
    //
    // https://www.rfc-editor.org/rfc/rfc6851#section-4.3
    // Servers supporting UIDPLUS [RFC4315] SHOULD send COPYUID in response to a UID MOVE command.
    //
    // https://www.rfc-editor.org/rfc/rfc4315#section-4
    // resp-text-code  =/ ... / resp-code-copy / ...
    #[no_eof]
    pub rule move_data() -> (usize, Move)
      = "OK" SP() "[" c:resp_code_copy() "]" SP() text() CRLF() p:position!()
      { (p, c) }
  }
}

pub fn plain(user: &str, password: &str) -> String {
  let engine = base64::engine::GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    base64::engine::general_purpose::PAD,
  );
  // https://www.rfc-editor.org/rfc/rfc2595#section-6
  // Non-US-ASCII characters are permitted as long as they are represented in UTF-8.
  engine.encode(format!("\0{user}\0{password}"))
}

pub fn utf7_to_utf8(input: &[u8]) -> Option<String> {
  let engine = base64::engine::GeneralPurpose::new(
    &base64::alphabet::IMAP_MUTF7,
    base64::engine::general_purpose::NO_PAD,
  );
  let mut buffer = Vec::new();
  let mut output = String::new();
  let mut i = 0;
  while i < input.len() {
    match input[i] {
      // https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3
      // "&" is used to shift to modified BASE64 and "-" to shift back to US-ASCII.
      b'&' => {
        let start = i;
        loop {
          i += 1;
          if i == input.len() {
            return None;
          }
          if input[i] == b'-' {
            break;
          }
        }
        if start + 1 == i {
          // https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3
          // The character "&" (0x26) is represented by the two-octet sequence "&-".
          output.push('&');
        } else {
          // https://www.rfc-editor.org/rfc/rfc2152
          // Unicode is encoded using Modified Base64 by first converting Unicode 16-bit quantities
          // to an octet stream (with the most significant octet first).
          buffer.truncate(0);
          buffer
            .try_reserve(base64::decoded_len_estimate(i - (start + 1)))
            .ok()?;
          engine.decode_vec(&input[start + 1..i], &mut buffer).ok()?;

          let mut decoder = encoding_rs::UTF_16BE.new_decoder_without_bom_handling();
          output
            .try_reserve(decoder.max_utf8_buffer_length_without_replacement(buffer.len())?)
            .ok()?;
          let (result, _) = decoder.decode_to_string_without_replacement(
            &buffer,
            &mut output,
            true, // last
          );
          match result {
            encoding_rs::DecoderResult::InputEmpty => (),
            _ => return None,
          }
        }
      }
      // https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3
      // In modified UTF-7, printable US-ASCII characters, except for "&", represent themselves;
      // that is, characters with octet values 0x20-0x25 and 0x27-0x7e.
      c @ 0x20..=0x25 | c @ 0x27..=0x7e => output.push(c as char),
      _ => return None,
    }
    i += 1;
  }
  Some(output)
}

fn escape(bytes: &[u8]) -> String {
  let mut string = String::new();
  for byte in bytes {
    string += &std::ascii::escape_default(*byte).to_string();
  }
  string
}

fn summarize(bytes: &[u8]) -> String {
  let stop = bytes
    .windows(2)
    .position(|window| window == b"\r\n")
    .unwrap_or(bytes.len());
  let stop = cmp::min(stop + 2 /* \r\n */, bytes.len());
  let mut string = escape(&bytes[..stop]);
  if stop < bytes.len() {
    string += "...omitted...";
  }
  string
}

#[derive(Debug)]
pub struct Stream<RW> {
  rw: RW,
  buffer: Vec<u8>,
  end: cell::Cell<usize>,
  needle: Option<String>,
}

impl<RW> Stream<RW>
where
  RW: io::Read + io::Write,
{
  pub fn new(rw: RW) -> Self {
    Self {
      rw,
      buffer: Vec::new(),
      end: cell::Cell::new(0),
      needle: None,
    }
  }

  fn inner_input(&mut self, buffers: &[&[u8]], log: usize) -> anyhow::Result<()> {
    if log::log_enabled!(log::Level::Debug) && log > 0 {
      log::debug!(
        "> {}{}",
        escape(&buffers[..log].concat()),
        if log < buffers.len() {
          "...omitted..."
        } else {
          ""
        }
      );
    } else {
      log::debug!("> ...omitted...");
    }
    for buffer in buffers.iter() {
      // https://www.rfc-editor.org/rfc/rfc7162#section-4
      // [...] a client should limit the length of the command lines it generates to approximately
      // 8192 octets (including all quoted strings but not including literals).
      self.rw.write_all(buffer)?;
    }
    Ok(())
  }

  pub fn read(&mut self, buffer: &mut [u8]) -> anyhow::Result<usize> {
    match self.rw.read(buffer)? {
      0 => anyhow::bail!("end of stream"),
      length => {
        self.buffer.extend_from_slice(&buffer[..length]);
        Ok(length)
      }
    }
  }

  fn chunk(&mut self) -> anyhow::Result<()> {
    // PEG doesn't return any information whatsoever that could tell us we're making progress but
    // still failing the parse (for example, when transferring large messages):
    // https://github.com/kevinmehall/rust-peg/discussions/326
    // IMAP has no response length indication so it's probably impossible to reliably understand
    // responses without an exhaustive parser. Because I don't want to be in this business I'm
    // opting for something I'm gonna regret: introducing my own chunking protocol on top :)

    // Get rid of the previous chunk.
    if let Some(needle) = self.needle.take() {
      loop {
        match self.expect(parser::start)? {
          b"*" => self.expect(parser::skip)?,
          tag if tag == needle.as_bytes() => break self.expect(parser::ok)?,
          tag => anyhow::bail!("unexpected tag {tag:?}"),
        }
      }
    }

    // Start a new chunk.
    let needle = uuid::Uuid::new_v4().as_hyphenated().to_string();
    let command: &[&[u8]] = &[needle.as_bytes(), &b" NOOP\r\n"[..]];
    self.inner_input(command, command.len())?;

    let mut buffer = [0; 1024 * 1024];
    // Yeah, I'm completely breaking the abstraction... Let's hope it's sufficiently unique.
    let needle_ = &[b"\r\n", needle.as_bytes(), b" OK "].concat();
    let (mut start, mut next_start) = (0, 0);
    let position = loop {
      // Starting from the end of the buffer (and limiting to the last data retrieved) with memchr
      // makes a huge difference over the naive .windows().position(). While this sounds wasteful,
      // CPU time appears to be dominated by Xapian.
      if let Some(position) = memchr::memmem::rfind_iter(&self.buffer[start..], needle_).next() {
        break position;
      }
      start = cmp::max(next_start, needle_.len()) - needle_.len();
      next_start += self.read(&mut buffer)?;
    };
    // The needle was found but we might not have enough to read until the end of the response.
    while parser::ok(&self.buffer[start + position + 2 + needle.len() + 1..]).is_err() {
      self.read(&mut buffer)?;
    }

    self.needle = Some(needle);
    Ok(())
  }

  pub fn input(&mut self, buffers: &[&[u8]], log: usize) -> anyhow::Result<()> {
    let end = self.end.get();
    let rest = self.buffer.len() - end;
    self.buffer.copy_within(end.., 0);
    self.buffer.truncate(rest);
    self.end.set(0);

    self.inner_input(buffers, log)?;
    // IMAP allows for reordering pipelined commands, wait for some input first (I can't remember if
    // untagged responses can come any time besides the initial login).
    self.read(&mut [0; 1])?;
    self.chunk()
  }

  fn inner_parse<'a, P, R>(&'a self, parser: P) -> anyhow::Result<R>
  where
    P: Fn(
      &'a [u8],
    ) -> Result<(usize, R), peg::error::ParseError<<[u8] as ::peg::Parse>::PositionRepr>>,
  {
    let start = self.end.get();
    let buffer = &self.buffer[start..];
    match parser(buffer) {
      Ok((end, result)) => {
        log::debug!("< {}", summarize(&buffer[..end]));
        self.end.set(self.end.get() + end);
        Ok(result)
      }
      Err(error) => {
        log::trace!("<< {:?} {}", error, summarize(buffer));
        Err(error).context(summarize(buffer))?
      }
    }
  }

  pub fn parse<'a, P, R>(&'a self, parser: P) -> anyhow::Result<Option<R>>
  where
    P: Fn(
      &'a [u8],
    ) -> Result<(usize, R), peg::error::ParseError<<[u8] as ::peg::Parse>::PositionRepr>>,
  {
    match self.inner_parse(parser) {
      Ok(result) => Ok(Some(result)),
      Err(error) => {
        match error.downcast_ref::<peg::error::ParseError<<[u8] as ::peg::Parse>::PositionRepr>>() {
          Some(_) => Ok(None),
          None => Err(error),
        }
      }
    }
  }

  pub fn expect<'a, P, R>(&'a self, parser: P) -> anyhow::Result<R>
  where
    P: Fn(
      &'a [u8],
    ) -> Result<(usize, R), peg::error::ParseError<<[u8] as ::peg::Parse>::PositionRepr>>,
  {
    self.inner_parse(parser)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn utf7_to_ut8() {
    // https://www.rfc-editor.org/rfc/rfc3501#section-5.1.3
    assert_eq!("", utf7_to_utf8(b"").unwrap());
    assert_eq!("&", utf7_to_utf8(b"&-").unwrap());
    // [...] a mailbox name which mixes English, Chinese, and Japanese text:
    assert_eq!(
      "~peter/mail/台北/日本語",
      utf7_to_utf8(b"~peter/mail/&U,BTFw-/&ZeVnLIqe-").unwrap()
    );
    // [...] the string "&Jjo!" is not a valid mailbox name because it does not contain a shift to
    // US-ASCII before the "!".
    assert_eq!(None, utf7_to_utf8(b"&Jjo!"));
    // The correct form is "&Jjo-!".
    assert_eq!("☺!", utf7_to_utf8(b"&Jjo-!").unwrap());
    // The string "&U,BTFw-&ZeVnLIqe-" is not permitted because it contains a superfluous shift.
    // (However, the implementation allows it for simplicity as it shouldn't be detrimental).
    assert_eq!("台北日本語", utf7_to_utf8(b"&U,BTFw-&ZeVnLIqe-").unwrap());
    // The correct form is "&U,BTF2XlZyyKng-".
    assert_eq!("台北日本語", utf7_to_utf8(b"&U,BTF2XlZyyKng-").unwrap())
  }

  #[test]
  fn start() {
    let (_, untagged) = parser::start(b"* ").unwrap();
    assert_eq!(b"*", untagged);

    let (_, tag) = parser::start(b"tag ").unwrap();
    assert_eq!(b"tag", tag);
  }

  #[test]
  fn available_capabilities() {
    let (_, capabilities) =
      parser::available_capabilities(b"OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] Dovecot ready.\r\n")
        .unwrap();
    assert_eq!(vec![&b"IMAP4rev1"[..], &b"AUTH=PLAIN"[..]], capabilities);
  }

  #[test]
  fn enabled_capabilities() {
    let (_, capabilities) = parser::enabled_capabilities(b"ENABLED CONDSTORE\r\n").unwrap();
    assert_eq!(vec![b"CONDSTORE"], capabilities);
  }

  #[test]
  fn list_mailbox() {
    let (_, (flags, seperator, mailbox)) =
      parser::list_mailbox(b"LIST (\\flag1 \\flag2) \"/\" \"quoted\"\r\n").unwrap();
    assert_eq!(vec![b"\\flag1", b"\\flag2"], flags);
    assert_eq!(Some(b'/'), seperator);
    assert_eq!(
      Mailbox::Other(borrow::Cow::Owned((&b"quoted"[..]).into())),
      mailbox
    );

    let (_, (_, _, mailbox)) =
      parser::list_mailbox(b"LIST (\\flag1 \\flag2) \"/\" {7}\r\nliteral\r\n").unwrap();
    assert_eq!(Mailbox::Other(borrow::Cow::Borrowed(b"literal")), mailbox);
  }

  #[test]
  fn select_data() {
    let (_, select) =
      parser::select_data(b"OK [PERMANENTFLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*)] Flags permitted.\r\n").unwrap();
    assert_eq!(
      Select::Flags(vec![
        b"\\Answered",
        b"\\Flagged",
        b"\\Deleted",
        b"\\Seen",
        b"\\Draft",
        b"\\*",
      ]),
      select
    );

    let (_, select) = parser::select_data(b"OK [UIDVALIDITY 1676645821] UIDs valid\r\n").unwrap();
    assert_eq!(Select::UIDValidity(1676645821), select);

    let (_, select) = parser::select_data(b"OK [HIGHESTMODSEQ 2] Highest\r\n").unwrap();
    assert_eq!(Select::HighestModSeq(2), select);

    let (_, select) = parser::select_data(b"VANISHED (EARLIER) 1:10\r\n").unwrap();
    assert_eq!(Select::Vanished(vec![Range(1, 10)]), select);

    for test in [
      b"1 FETCH (UID 10 FLAGS (\\Seen) MODSEQ (100))\r\n",
      b"1 FETCH (FLAGS (\\Seen) MODSEQ (100) UID 10)\r\n",
      b"1 FETCH (MODSEQ (100) UID 10 FLAGS (\\Seen))\r\n",
    ] {
      let (_, select) = parser::select_data(test).unwrap();
      assert_eq!(
        Select::Fetch(SelectFetch {
          uid: 10,
          flags: vec![b"\\Seen"],
          modseq: 100
        }),
        select
      );
    }
  }

  #[test]
  fn fetch_body_data() {
    let (_, fetch) = parser::fetch_body_data(b"1 FETCH (UID 10 BODY[] {0}\r\n)\r\n").unwrap();
    assert_eq!((10, Some(borrow::Cow::Borrowed(&b""[..]))), fetch);

    let (_, fetch) = parser::fetch_body_data(b"1 FETCH (BODY[] \"\" UID 10)\r\n").unwrap();
    assert_eq!((10, Some(borrow::Cow::Owned(b"".to_vec()))), fetch);
  }

  #[test]
  fn append() {
    let (_, append) = parser::append(b"OK [APPENDUID 1677851195 1] Append completed.\r\n").unwrap();
    assert_eq!(
      Append {
        uidvalidity: 1677851195,
        uid: 1
      },
      append
    );
  }

  #[test]
  fn append_data() {
    let (_, highestmodseq) = parser::append_data(b"OK [HIGHESTMODSEQ 3] Highest\r\n").unwrap();
    assert_eq!(highestmodseq, 3);
  }

  #[test]
  fn store() {
    let (_, uids) = parser::store(b"OK Store completed.\r\n").unwrap();
    assert_eq!(None, uids);

    let (_, uids) = parser::store(b"OK [MODIFIED 7,9] Conditional STORE failed\r\n").unwrap();
    assert_eq!(Some(vec![Range(7, 7), Range(9, 9)]), uids);
  }

  #[test]
  fn store_data() {
    let (_, store) = parser::store_data(b"1 FETCH (UID 1 MODSEQ (3))\r\n").unwrap();
    assert_eq!(Store { uid: 1, modseq: 3 }, store);
  }

  #[test]
  fn r#move() {
    let (_, highestmodseq) = parser::move_(b"OK Done\r\n").unwrap();
    assert_eq!(None, highestmodseq);

    let (_, highestmodseq) = parser::move_(b"OK [HIGHESTMODSEQ 4] Move completed.\r\n").unwrap();
    assert_eq!(Some(4), highestmodseq);
  }

  #[test]
  fn move_data() {
    let (_, r#move) = parser::move_data(b"OK [COPYUID 1677882317 1 1] Moved UIDs.\r\n").unwrap();
    assert_eq!(
      Move {
        uidvalidity: 1677882317,
        from: vec![Range(1, 1)],
        to: vec![Range(1, 1)]
      },
      r#move
    );
  }
}
