//! Puts the linker script on the link search path. Binaries add
//! `-C link-arg=-Tkumiki-p4v1.x` (see `firmware-p4/.cargo/config.toml`).

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=kumiki-p4v1.x");
    if env::var_os("CARGO_FEATURE_RT").is_none() {
        return; // the embedding program brings its own layout
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("kumiki-p4v1.x", out.join("kumiki-p4v1.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=kumiki-p4v1.x");
}
