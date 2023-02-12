use crate::common;
use anyhow::Context as _;
use std::{fs, io::Write as _, process};

pub fn server() -> anyhow::Result<(tempfile::TempDir, common::Child, u16)> {
  let directory = tempfile::tempdir()?;
  let base_dir = directory
    .path()
    .to_str()
    .with_context(|| "invalid directory")?;
  let unix_user =
    users::get_user_by_uid(users::get_current_uid()).with_context(|| "invalid user")?;
  let user = unix_user.name().to_str().with_context(|| "invalid user")?;
  let unix_group =
    users::get_group_by_gid(users::get_current_gid()).with_context(|| "invalid group")?;
  let group = unix_group
    .name()
    .to_str()
    .with_context(|| "invalid group")?;
  let port = portpicker::pick_unused_port().with_context(|| "no free port")?;

  let passwd = directory.path().join("test.passwd");
  let mut file = fs::File::create(&passwd)?;
  file.write_all(
    format!(
      "# https://doc.dovecot.org/configuration_manual/authentication/passwd_file/
user:{{plain}}password:::default user for tests:::
user1:{{plain}}password:::multi-user test:::
user2:{{plain}}password:::multi-user test:::
user3:{{plain}}password:::multi-user test:::
separator:{{plain}}password:::separator test:::userdb_namespace/default/separator=.
maildir:{{plain}}password:::static maildir benchmark:::userdb_mail=maildir:/tmp/maildir
"
    )
    .as_bytes(),
  )?;
  file.sync_all()?;
  let passwd = passwd.to_str().unwrap();

  // Can't name it dovecot.conf because this will be a symlink to the configuration.
  let configuration = directory.path().join("test.conf");
  let mut file = fs::File::create(&configuration)?;
  file.write_all(
    format!(
      "# https://wiki.dovecot.org/HowTo/Rootless
base_dir = {base_dir}
state_dir = $base_dir
default_login_user = {user}
default_internal_user = {user}
default_internal_group = {group}

log_path = {base_dir}/test.log
auth_debug_passwords = yes
mail_debug = yes

protocols = imap
ssl = no

service anvil {{
  chroot =
}}
service imap-login {{
  chroot =
  inet_listener imap {{
    port = {port}
  }}
}}

passdb {{
  driver = passwd-file
  # Some sources say nodelay is supposed to remove the 2s penaly but I can't get it to work
  # https://doc.dovecot.org/configuration_manual/authentication/password_database_extra_fields
  # https://doc.dovecot.org/configuration_manual/authentication/auth_penalty/
  args = username_format=%n {passwd}
}}
userdb {{
  driver = passwd-file
  # https://doc.dovecot.org/configuration_manual/authentication/passwd/#passwd
  args = username_format=%n {passwd}
  default_fields = uid={user} gid={group} home={base_dir}/%u/home mail=maildir:{base_dir}/%u/maildir
}}
namespace default {{
  inbox = yes
  separator = /
}}
"
    )
    .as_bytes(),
  )?;
  file.sync_all()?;

  log::debug!("running dovecot from {base_dir} on port {port}");
  let child = process::Command::new("dovecot")
    .args(&[
      "-Fc",
      configuration.to_str().with_context(|| "invalid file")?,
    ])
    .spawn()?;
  Ok((directory, common::Child(child), port))
}
