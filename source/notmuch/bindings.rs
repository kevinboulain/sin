// notmuch-rs doesn't really provide a safe interface
// (https://github.com/vhdirk/notmuch-rs/issues/24) and I need to wrap most of it anyway.

#![allow(clippy::let_unit_value)] // On purpose to catch API changes.

use std::{
  collections, convert, error, ffi, fmt, marker, ops, os::unix::ffi::OsStrExt as _, path, ptr, str,
};

#[allow(dead_code)]
#[allow(deref_nullptr)] // https://github.com/rust-lang/rust-bindgen/issues/1651
#[allow(non_camel_case_types)]
#[allow(non_snake_case)]
#[allow(non_upper_case_globals)]
mod private {
  include!(concat!(env!("OUT_DIR"), "/notmuch.rs"));
}

#[derive(Debug)]
pub enum Error {
  Status(private::notmuch_status_t),
  UTF8(str::Utf8Error),
}

impl Error {
  pub fn no_database(&self) -> bool {
    matches!(
      self,
      Error::Status(private::notmuch_status_t_NOTMUCH_STATUS_NO_DATABASE)
    )
  }

  pub fn file_error(&self) -> bool {
    matches!(
      self,
      Error::Status(private::notmuch_status_t_NOTMUCH_STATUS_FILE_ERROR)
    )
  }
}

impl convert::From<str::Utf8Error> for Error {
  fn from(error: str::Utf8Error) -> Self {
    Error::UTF8(error)
  }
}

impl fmt::Display for Error {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
    match self {
      Error::Status(status) => {
        let cstr = unsafe { ffi::CStr::from_ptr(private::notmuch_status_to_string(*status)) };
        write!(formatter, "{:?}", cstr)
      }
      Error::UTF8(error) => write!(formatter, "{}", error),
    }
  }
}

impl error::Error for Error {}

// https://doc.rust-lang.org/std/ffi/struct.CStr.html#method.as_ptr
// It is your responsibility to make sure that the underlying memory is not freed too early.
fn str_to_cstring(str: &str) -> Result<ffi::CString, Error> {
  match ffi::CString::new(str) {
    Ok(cstring) => Ok(cstring),
    Err(_) => Err(Error::Status(
      private::notmuch_status_t_NOTMUCH_STATUS_ILLEGAL_ARGUMENT,
    )),
  }
}

fn path_to_cstring(path: &path::Path) -> Result<ffi::CString, Error> {
  if let Some(str) = path.to_str() {
    return str_to_cstring(str);
  }
  Err(Error::Status(
    private::notmuch_status_t_NOTMUCH_STATUS_ILLEGAL_ARGUMENT,
  ))
}

#[derive(Debug)]
pub struct Database(*mut private::notmuch_database_t);

impl ops::Drop for Database {
  fn drop(&mut self) {
    // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
    // Commit changes and close the given notmuch database.
    // notmuch_database_close can be called multiple times. Later calls have no effect.
    if let Err(error) = self.close() {
      log::warn!("couldn't close database {error}")
    }
  }
}

impl Database {
  pub fn open(path: Option<&path::Path>) -> Result<Self, Error> {
    let path = match path {
      Some(path) => Some(path_to_cstring(path)?),
      None => None,
    };
    let path = path
      .as_ref() // Avoid freeing the CString...
      .map(|p| p.as_ptr())
      .unwrap_or(ptr::null());
    let mut database = ptr::null_mut();
    match unsafe {
      private::notmuch_database_open_with_config(
        path,
        private::notmuch_database_mode_t_NOTMUCH_DATABASE_MODE_READ_WRITE,
        // Load the user's configuration (as opposed to --config ''): try to respect user settings but
        // note that new.tags can't really be enforced.
        ptr::null(),
        // Use the user's profile.
        ptr::null(),
        &mut database,
        // No error message needed?
        ptr::null_mut(),
      )
    } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(Self(database)),
      status => Err(Error::Status(status)),
    }
  }

  pub fn reopen(&mut self) -> Result<(), Error> {
    match unsafe {
      private::notmuch_database_reopen(
        self.0,
        private::notmuch_database_mode_t_NOTMUCH_DATABASE_MODE_READ_WRITE,
      )
    } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn create(path: &path::Path) -> Result<Self, Error> {
    let path = path_to_cstring(path)?;
    let mut database = ptr::null_mut();
    match unsafe {
      private::notmuch_database_create_with_config(
        path.as_ptr(),
        // Load the user's configuration (as opposed to --config ''): try to respect user settings but
        // note that new.tags can't really be enforced.
        ptr::null(),
        // Use the user's profile.
        ptr::null(),
        &mut database,
        // No error message needed?
        ptr::null_mut(),
      )
    } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(Self(database)),
      status => Err(Error::Status(status)),
    }
  }

  pub fn close(&mut self) -> Result<(), Error> {
    match unsafe { private::notmuch_database_close(self.0) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn begin_atomic(&mut self) -> Result<(), Error> {
    match unsafe { private::notmuch_database_begin_atomic(self.0) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn end_atomic(&mut self) -> Result<(), Error> {
    match unsafe { private::notmuch_database_end_atomic(self.0) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn query(&'_ self, query: &str) -> Result<Messages<'_>, Error> {
    let query = str_to_cstring(query)?;
    let query = unsafe { private::notmuch_query_create(self.0, query.as_ptr()) };
    if query.is_null() {
      return Err(Error::Status(
        private::notmuch_status_t_NOTMUCH_STATUS_OUT_OF_MEMORY,
      ));
    }
    let () = unsafe {
      private::notmuch_query_set_omit_excluded(
        query,
        private::notmuch_exclude_t_NOTMUCH_EXCLUDE_FALSE, // For idempotency.
      )
    };
    let mut messages = ptr::null_mut();
    match unsafe { private::notmuch_query_search_messages(query, &mut messages) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => (),
      status => return Err(Error::Status(status)),
    }
    // NULL is handled by notmuch_messages_valid.
    Ok(Messages(query, messages, marker::PhantomData))
  }

  // This doesn't look like it needs to be mut: it won't invalidate existing messages.
  pub fn index_message(&'_ self, path: &path::Path) -> Result<Message<'_>, Error> {
    let path = path_to_cstring(path)?;
    let mut message = ptr::null_mut();
    match unsafe {
      private::notmuch_database_index_file(self.0, path.as_ptr(), ptr::null_mut(), &mut message)
    } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS
      | private::notmuch_status_t_NOTMUCH_STATUS_DUPLICATE_MESSAGE_ID => (),
      status => return Err(Error::Status(status)),
    };
    assert!(!message.is_null());
    Ok(Message(message, marker::PhantomData))
  }

  // This doesn't look like it needs to be mut: it won't invalidate existing messages.
  pub fn remove_message(&'_ self, path: &path::Path) -> Result<(), Error> {
    let path = path_to_cstring(path)?;
    match unsafe { private::notmuch_database_remove_message(self.0, path.as_ptr()) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS
      | private::notmuch_status_t_NOTMUCH_STATUS_DUPLICATE_MESSAGE_ID => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn find_message_by_filename(
    &'_ self,
    path: &path::Path,
  ) -> Result<Option<Message<'_>>, Error> {
    let path = path_to_cstring(path)?;
    let mut message = ptr::null_mut();
    match unsafe {
      private::notmuch_database_find_message_by_filename(self.0, path.as_ptr(), &mut message)
    } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => (),
      status => return Err(Error::Status(status)),
    };
    Ok(match message.is_null() {
      true => None,
      false => Some(Message(message, marker::PhantomData)),
    })
  }

  pub fn path(&self) -> &path::Path {
    let osstr: &ffi::OsStr = unsafe {
      // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
      // The return value is a string owned by notmuch so should not be modified nor freed by the
      // caller.
      let path = private::notmuch_database_get_path(self.0);
      assert!(!path.is_null());
      ffi::OsStr::from_bytes(ffi::CStr::from_ptr(path).to_bytes())
    };
    path::Path::new(osstr)
  }

  pub fn lastmod(&self) -> u64 {
    unsafe { private::notmuch_database_get_revision(self.0, ptr::null_mut()) }
  }
}

#[derive(Debug)]
pub struct Messages<'a>(
  *mut private::notmuch_query_t,
  *mut private::notmuch_messages_t,
  marker::PhantomData<&'a ()>,
);

impl<'a> ops::Drop for Messages<'a> {
  fn drop(&mut self) {
    let () = unsafe { private::notmuch_query_destroy(self.0) };
  }
}

impl<'a> Messages<'a> {
  pub fn next(&'_ mut self) -> Option<Message<'_>> {
    // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
    // When this function returns TRUE, notmuch_messages_get will return a valid object. Whereas
    // when this function returns FALSE, notmuch_messages_get will return NULL.
    match unsafe { private::notmuch_messages_valid(self.1) } {
      0 => None,
      _ => {
        let message = unsafe { private::notmuch_messages_get(self.1) };
        assert!(!message.is_null());
        // Safe: doesn't invalidate anything yet.
        let () = unsafe { private::notmuch_messages_move_to_next(self.1) };
        Some(Message(message, marker::PhantomData))
      }
    }
  }
}

#[derive(Debug)]
pub struct Message<'a>(*mut private::notmuch_message_t, marker::PhantomData<&'a ()>);

impl<'a> ops::Drop for Message<'a> {
  fn drop(&mut self) {
    // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
    // If you are finished with a message before its containing query, you can call
    // notmuch_message_destroy to clean up some memory sooner [...]. Otherwise, if your message
    // objects are long-lived, then you don't need to call notmuch_message_destroy and all the
    // memory will still be reclaimed when the query is destroyed.
    let () = unsafe { private::notmuch_message_destroy(self.0) };
  }
}

impl<'a> Message<'a> {
  pub fn properties(&'_ self, key: &str, exact: bool) -> Result<Properties<'_>, Error> {
    let key = str_to_cstring(key)?;
    // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
    // The notmuch_message_properties_t object is owned by the message and as such, will only be
    // valid for as long as the message is valid, (which is until the query from which it derived is
    // destroyed).
    let properties =
      unsafe { private::notmuch_message_get_properties(self.0, key.as_ptr(), exact.into()) };
    // NULL isn't handled by notmuch_message_properties_valid.
    if properties.is_null() {
      return Err(Error::Status(
        private::notmuch_status_t_NOTMUCH_STATUS_OUT_OF_MEMORY,
      ));
    }
    Ok(Properties(properties, marker::PhantomData))
  }

  pub fn add_property(&mut self, key: &str, value: &str) -> Result<(), Error> {
    let key = str_to_cstring(key)?;
    let value = str_to_cstring(value)?;
    match unsafe { private::notmuch_message_add_property(self.0, key.as_ptr(), value.as_ptr()) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn remove_property(&mut self, key: &str, value: &str) -> Result<(), Error> {
    let key = str_to_cstring(key)?;
    let value = str_to_cstring(value)?;
    match unsafe { private::notmuch_message_remove_property(self.0, key.as_ptr(), value.as_ptr()) }
    {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn remove_all_properties(&mut self, key: &str) -> Result<(), Error> {
    let key = str_to_cstring(key)?;
    match unsafe { private::notmuch_message_remove_all_properties(self.0, key.as_ptr()) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn remove_all_properties_with_prefix(&mut self, prefix: &str) -> Result<(), Error> {
    let prefix = str_to_cstring(prefix)?;
    match unsafe {
      private::notmuch_message_remove_all_properties_with_prefix(self.0, prefix.as_ptr())
    } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn tags(&'_ self) -> Result<collections::HashSet<&'_ str>, Error> {
    let mut tags = collections::HashSet::new();
    let tags_ = unsafe { private::notmuch_message_get_tags(self.0) };
    // NULL is handled by notmuch_tags_valid.
    while unsafe { private::notmuch_tags_valid(tags_) } != 0 {
      // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
      // The tags object is owned by the message and as such, will only be valid for as long as the
      // message is valid, (which is until the query from which it derived is destroyed).
      let tag = unsafe {
        let tag = private::notmuch_tags_get(tags_);
        ffi::CStr::from_ptr(tag)
      };
      tags.insert(tag.to_str()?);
      let () = unsafe { private::notmuch_tags_move_to_next(tags_) };
    }
    Ok(tags)
  }

  pub fn add_tag(&mut self, tag: &str) -> Result<(), Error> {
    let tag = str_to_cstring(tag)?;
    match unsafe { private::notmuch_message_add_tag(self.0, tag.as_ptr()) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn remove_tag(&mut self, tag: &str) -> Result<(), Error> {
    let tag = str_to_cstring(tag)?;
    match unsafe { private::notmuch_message_remove_tag(self.0, tag.as_ptr()) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn tags_to_maildir_flags(&mut self) -> Result<(), Error> {
    match unsafe { private::notmuch_message_tags_to_maildir_flags(self.0) } {
      private::notmuch_status_t_NOTMUCH_STATUS_SUCCESS => Ok(()),
      status => Err(Error::Status(status)),
    }
  }

  pub fn id(&'_ self) -> Result<&'_ str, Error> {
    // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
    // The returned string belongs to 'message' and as such, should not be modified by the caller
    // and will only be valid for as long as the message is valid, (which is until the query from
    // which it derived is destroyed).
    let id = unsafe { private::notmuch_message_get_message_id(self.0) };
    if id.is_null() {
      // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
      // This function will return NULL if triggers an unhandled Xapian exception.
      return Err(Error::Status(
        private::notmuch_status_t_NOTMUCH_STATUS_XAPIAN_EXCEPTION,
      ));
    }
    Ok(unsafe { ffi::CStr::from_ptr(id) }.to_str()?)
  }

  pub fn paths(&self) -> Result<Vec<path::PathBuf>, Error> {
    // It looks like we need to return a copy, metadata invalidation will purge filenames from the
    // message.
    let mut paths = Vec::new();
    let paths_ = unsafe { private::notmuch_message_get_filenames(self.0) };
    // NULL is handled by notmuch_tags_valid.
    while unsafe { private::notmuch_filenames_valid(paths_) } != 0 {
      let path = unsafe {
        let path = private::notmuch_filenames_get(paths_);
        assert!(!path.is_null());
        ffi::OsStr::from_bytes(ffi::CStr::from_ptr(path).to_bytes())
      };
      paths.push(path::Path::new(path).to_path_buf());
      let () = unsafe { private::notmuch_filenames_move_to_next(paths_) };
    }
    Ok(paths)
  }
}

#[derive(Debug)]
pub struct Properties<'a>(
  *mut private::notmuch_message_properties_t,
  marker::PhantomData<&'a ()>,
);

impl<'a> ops::Drop for Properties<'a> {
  fn drop(&mut self) {
    let () = unsafe { private::notmuch_message_properties_destroy(self.0) };
  }
}

impl<'a> Properties<'a> {
  pub fn next(&mut self) -> Result<Option<(&'a str, &'a str)>, Error> {
    match unsafe { private::notmuch_message_properties_valid(self.0) } {
      0 => Ok(None),
      _ => {
        let (key, value) = unsafe {
          (
            private::notmuch_message_properties_key(self.0),
            private::notmuch_message_properties_value(self.0),
          )
        };
        assert!(!key.is_null() && !value.is_null());
        // Safe: doesn't invalidate anything yet.
        let () = unsafe { private::notmuch_message_properties_move_to_next(self.0) };
        Ok(Some((
          unsafe { ffi::CStr::from_ptr(key) }.to_str()?,
          unsafe { ffi::CStr::from_ptr(value) }.to_str()?,
        )))
      }
    }
  }
}
