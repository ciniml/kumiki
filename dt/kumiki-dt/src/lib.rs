//! # kumiki-dt
//!
//! Device Tree Source handling in pure Rust: a small C preprocessor
//! ([`pp`]), a DTS v1 parser with node merging, `&label` overrides and
//! `/delete-node/` ([`parse`]), and a tree model with label / phandle
//! resolution ([`tree`]). No `dtc` or `cpp` is needed at build time; the
//! accepted subset is plain DTS, so the same files also compile with `dtc`.

pub mod parse;
pub mod pp;
pub mod tree;

use std::path::{Path, PathBuf};

pub use tree::{Cell, Node, Property, Tree, Value};

/// Error with a human-readable message (file / line when known).
#[derive(Debug, Clone)]
pub struct Error(String);

impl Error {
    /// Create an error.
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Preprocess and parse a DTS file. `include_dirs` serve `#include <…>`.
/// Returns the tree and the list of files read.
pub fn load(path: &Path, include_dirs: &[PathBuf]) -> Result<(Tree, Vec<PathBuf>), Error> {
    let mut pp = pp::Preprocessor::new(include_dirs.to_vec());
    let text = pp.run_file(path)?;
    let tree = parse::parse(&text).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    Ok((tree, pp.files))
}
