use std::{env, path};

// https://rust-lang.github.io/rust-bindgen/tutorial-3.html
fn main() {
  let out = path::PathBuf::from(env::var("OUT_DIR").unwrap());
  let header = "source/notmuch/bindings.h";
  println!("cargo:rustc-link-lib=notmuch");
  println!("cargo:rerun-if-changed={}", header);
  let bindings = bindgen::Builder::default()
    .header(header)
    .parse_callbacks(Box::new(bindgen::CargoCallbacks))
    .generate()
    .unwrap();
  bindings.write_to_file(out.join("notmuch.rs")).unwrap();
}
