let
  pkgs = import <nixpkgs> {
    overlays = [
      (self: super: {
        xapian = super.enableDebugging (super.xapian.overrideAttrs (old: {
          doCheck = false;
        }));
        notmuch = super.enableDebugging (super.notmuch.overrideAttrs (old: {
          doCheck = false;
          patches = [
            ./patches/notmuch-01.patch
            ./patches/notmuch-02.patch
            ./patches/notmuch-03.patch
            ./patches/notmuch-04.patch
            ./patches/notmuch-05.patch
          ];
        }));
      })
      (self: super: {
        dovecot = super.dovecot.overrideAttrs (old: {
          doCheck = false;
          # Remove most patches, especially 2.3.x-module_dir.patch which
          # hardcodes /etc/dovecot/modules and makes it impossible to start
          # dovecot as non-root...
          # https://github.com/NixOS/nixpkgs/issues/158182
          patches = [
            (super.fetchpatch {
              url = "https://salsa.debian.org/debian/dovecot/-/raw/debian/1%252.3.19.1+dfsg1-2/debian/patches/Support-openssl-3.0.patch";
              hash = "sha256-PbBB1jIY3jIC8Js1NY93zkV0gISGUq7Nc67Ul5tN7sw=";
            })
          ];
        });
      })
    ];
  };
  fenix = (import (fetchTarball "https://github.com/nix-community/fenix/archive/main.tar.gz") {}).stable;
in
pkgs.mkShell {
  RUST_BACKTRACE = 1;
  shellHook = ''
    imap-plain-pass() {
      # imap-plain-pass user password-store-entry
      { echo -ne "\0''${1?}\0" && echo -n "$(pass "''${2?}" | head -1)"; } | base64 --wrap=0
      echo
    }
    imap-shell() {
      # imap-shell ssl:host:993
      # imap-shell tcp:host:143
      socat readline,crlf "''${1?}"
    }
  '';
  nativeBuildInputs = with pkgs; [
    # https://gist.github.com/yihuang/b874efb97e99d4b6d12bf039f98ae31e?permalink_comment_id=4311076#gistcomment-4311076
    rustPlatform.bindgenHook
    # Toolchain.
    (fenix.withComponents [
      "cargo"
      "clippy"
      # Only available under the 'complete' (nightly) toolchain, not 'stable'.
      # Won't work anyway because we call into libnotmuch.
      # "miri"
      "rust-analyzer"
      "rust-src"
      "rustc"
      "rustfmt"
    ])
    # Cargo goodies.
    cargo-edit
    cargo-expand
    cargo-flamegraph
    cargo-tarpaulin
  ];
  buildInputs = with pkgs; [
    # Dependencies.
    notmuch
    # Tests.
    dovecot
    # Debugging.
    gdb
    socat
  ];
}
