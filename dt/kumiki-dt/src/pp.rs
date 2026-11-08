//! Minimal C preprocessor for DTS files.
//!
//! Supports what board descriptions need: `#include "…"` / `#include <…>`,
//! object-like and function-like `#define`, `#undef`, `#ifdef` / `#ifndef` /
//! `#else` / `#endif`, and `//` / `/* */` comments. Lines starting with `#`
//! that are not directives (e.g. `#address-cells = <1>;`) are kept, like
//! `cpp -x assembler-with-cpp` does for Linux and Zephyr DTS files.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::Error;

#[derive(Clone, Debug)]
struct Macro {
    params: Option<Vec<String>>,
    body: String,
}

/// Preprocessor state.
pub struct Preprocessor {
    include_dirs: Vec<PathBuf>,
    macros: HashMap<String, Macro>,
    /// Files read, in order (for build-script `rerun-if-changed`).
    pub files: Vec<PathBuf>,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Remove comments, keeping newlines (so line numbers stay meaningful) and strings intact.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(b[i]);
                    i += 1;
                }
                out.push(b[i]);
                i += 1;
            }
            if i < b.len() {
                out.push('"');
                i += 1;
            }
        } else if c == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                if b[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

impl Preprocessor {
    /// Create a preprocessor searching `<…>` includes in `include_dirs`.
    pub fn new(include_dirs: Vec<PathBuf>) -> Self {
        Self { include_dirs, macros: HashMap::new(), files: Vec::new() }
    }

    /// Predefine an object-like macro.
    pub fn define(&mut self, name: &str, value: &str) {
        self.macros.insert(name.into(), Macro { params: None, body: value.into() });
    }

    /// Preprocess a file.
    pub fn run_file(&mut self, path: &Path) -> Result<String, Error> {
        let src = std::fs::read_to_string(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        self.files.push(path.to_path_buf());
        self.run(&src, path.parent().unwrap_or(Path::new(".")), &path.display().to_string(), 0)
    }

    /// Preprocess source text (`dir` resolves `#include "…"`).
    pub fn run(&mut self, src: &str, dir: &Path, name: &str, depth: usize) -> Result<String, Error> {
        if depth > 32 {
            return Err(Error::new(format!("{name}: #include nested too deeply")));
        }
        let src = strip_comments(src);
        let mut out = String::new();
        // Stack of (active, any_branch_taken)
        let mut cond: Vec<(bool, bool)> = Vec::new();
        let active = |c: &Vec<(bool, bool)>| c.iter().all(|(a, _)| *a);
        let mut lines = src.lines().enumerate().peekable();
        while let Some((ln, line)) = lines.next() {
            // Line continuation.
            let mut line = line.to_string();
            while line.ends_with('\\') {
                line.pop();
                match lines.next() {
                    Some((_, l)) => line.push_str(l),
                    None => break,
                }
            }
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix('#') {
                let rest = rest.trim_start();
                let (kw, arg) = match rest.find(|c: char| c.is_whitespace()) {
                    Some(i) => (&rest[..i], rest[i..].trim()),
                    None => (rest, ""),
                };
                let at = || format!("{name}:{}", ln + 1);
                match kw {
                    "ifdef" | "ifndef" => {
                        let def = self.macros.contains_key(arg);
                        let take = if kw == "ifdef" { def } else { !def };
                        cond.push((take, take));
                        out.push('\n');
                        continue;
                    }
                    "if" => {
                        let take = arg.trim() != "0";
                        cond.push((take, take));
                        out.push('\n');
                        continue;
                    }
                    "else" => {
                        let top = cond.last_mut().ok_or_else(|| Error::new(format!("{}: #else without #if", at())))?;
                        top.0 = !top.1;
                        top.1 = true;
                        out.push('\n');
                        continue;
                    }
                    "endif" => {
                        cond.pop().ok_or_else(|| Error::new(format!("{}: #endif without #if", at())))?;
                        out.push('\n');
                        continue;
                    }
                    _ => {}
                }
                if !active(&cond) {
                    out.push('\n');
                    continue;
                }
                match kw {
                    "include" => {
                        let (file, local) = if let Some(f) = arg.strip_prefix('"') {
                            (f.trim_end_matches('"'), true)
                        } else if let Some(f) = arg.strip_prefix('<') {
                            (f.trim_end_matches('>'), false)
                        } else {
                            return Err(Error::new(format!("{}: bad #include", at())));
                        };
                        let mut cands = Vec::new();
                        if local {
                            cands.push(dir.join(file));
                        }
                        cands.extend(self.include_dirs.iter().map(|d| d.join(file)));
                        let path = cands
                            .into_iter()
                            .find(|p| p.is_file())
                            .ok_or_else(|| Error::new(format!("{}: cannot find include {file}", at())))?;
                        let text = std::fs::read_to_string(&path)
                            .map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
                        self.files.push(path.clone());
                        let sub_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                        let expanded = self.run(&text, &sub_dir, &path.display().to_string(), depth + 1)?;
                        out.push_str(&expanded);
                        out.push('\n');
                        continue;
                    }
                    "define" => {
                        self.parse_define(arg).map_err(|e| Error::new(format!("{}: {e}", at())))?;
                        out.push('\n');
                        continue;
                    }
                    "undef" => {
                        self.macros.remove(arg);
                        out.push('\n');
                        continue;
                    }
                    _ => {} // not a directive (#address-cells …): fall through as text
                }
            }
            if active(&cond) {
                out.push_str(&self.expand(&line, 0)?);
            }
            out.push('\n');
        }
        if !cond.is_empty() {
            return Err(Error::new(format!("{name}: unterminated #if")));
        }
        Ok(out)
    }

    fn parse_define(&mut self, arg: &str) -> Result<(), String> {
        let name_end = arg.find(|c: char| !is_ident_char(c)).unwrap_or(arg.len());
        let name = &arg[..name_end];
        if name.is_empty() {
            return Err("bad #define".into());
        }
        let rest = &arg[name_end..];
        if let Some(r) = rest.strip_prefix('(') {
            let close = r.find(')').ok_or("unterminated macro parameter list")?;
            let params = r[..close].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            self.macros.insert(name.into(), Macro { params: Some(params), body: r[close + 1..].trim().into() });
        } else {
            self.macros.insert(name.into(), Macro { params: None, body: rest.trim().into() });
        }
        Ok(())
    }

    /// Expand macros in one line (identifiers outside string literals).
    fn expand(&self, line: &str, depth: usize) -> Result<String, Error> {
        if depth > 32 {
            return Err(Error::new("macro expansion too deep"));
        }
        let c: Vec<char> = line.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        let mut changed = false;
        while i < c.len() {
            if c[i] == '"' {
                out.push('"');
                i += 1;
                while i < c.len() && c[i] != '"' {
                    out.push(c[i]);
                    i += 1;
                }
                if i < c.len() {
                    out.push('"');
                    i += 1;
                }
                continue;
            }
            // Identifiers glued to DTS name characters (e.g. `gpio-controller`,
            // `#gpio-cells`, `&label`) are not macro candidates when preceded by
            // `-`, `#`, `&`, `,` or `.`.
            let prev_blocks = i > 0 && matches!(c[i - 1], '-' | '#' | '&' | ',' | '.' | '@') || (i > 0 && is_ident_char(c[i - 1]));
            if is_ident_start(c[i]) && !prev_blocks {
                let s = i;
                while i < c.len() && is_ident_char(c[i]) {
                    i += 1;
                }
                let ident: String = c[s..i].iter().collect();
                let next_is_name = i < c.len() && matches!(c[i], '-' | ',');
                match self.macros.get(&ident) {
                    Some(m) if !next_is_name => match &m.params {
                        None => {
                            out.push_str(&m.body);
                            changed = true;
                        }
                        Some(params) => {
                            let mut j = i;
                            while j < c.len() && c[j] == ' ' {
                                j += 1;
                            }
                            if j < c.len() && c[j] == '(' {
                                let (args, end) = split_args(&c, j)?;
                                let mut body = m.body.clone();
                                for (p, a) in params.iter().zip(args.iter()) {
                                    body = replace_ident(&body, p, a.trim());
                                }
                                out.push_str(&body);
                                changed = true;
                                i = end;
                            } else {
                                out.push_str(&ident);
                            }
                        }
                    },
                    _ => out.push_str(&ident),
                }
                continue;
            }
            out.push(c[i]);
            i += 1;
        }
        if changed { self.expand(&out, depth + 1) } else { Ok(out) }
    }
}

fn split_args(c: &[char], open: usize) -> Result<(Vec<String>, usize), Error> {
    let mut depth = 0;
    let mut args = vec![String::new()];
    let mut i = open;
    while i < c.len() {
        let ch = c[i];
        match ch {
            '(' => {
                depth += 1;
                if depth > 1 {
                    args.last_mut().unwrap().push(ch);
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((args, i + 1));
                }
                args.last_mut().unwrap().push(ch);
            }
            ',' if depth == 1 => args.push(String::new()),
            _ => args.last_mut().unwrap().push(ch),
        }
        i += 1;
    }
    Err(Error::new("unterminated macro arguments"))
}

fn replace_ident(body: &str, ident: &str, with: &str) -> String {
    let c: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if is_ident_start(c[i]) && (i == 0 || !is_ident_char(c[i - 1])) {
            let s = i;
            while i < c.len() && is_ident_char(c[i]) {
                i += 1;
            }
            let w: String = c[s..i].iter().collect();
            out.push_str(if w == ident { with } else { &w });
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(src: &str) -> String {
        Preprocessor::new(vec![]).run(src, Path::new("."), "t", 0).unwrap()
    }

    #[test]
    fn defines_and_comments() {
        let out = pp("#define LOW 1 /* c */\n#define PIN(n) (n + 1)\nx = <&gpio 3 LOW>; // c\ny = <PIN(4)>;\n");
        assert!(out.contains("x = <&gpio 3 1>;"), "{out}");
        assert!(out.contains("y = <(4 + 1)>;"), "{out}");
    }

    #[test]
    fn non_directive_hash_lines_survive() {
        let out = pp("#define LOW 1\n\t#address-cells = <1>;\n gpio-controller;\n");
        assert!(out.contains("#address-cells = <1>;"));
        assert!(out.contains("gpio-controller;"));
    }

    #[test]
    fn ifdef_else() {
        let out = pp("#define A\n#ifdef A\nyes;\n#else\nno;\n#endif\n#ifndef A\nno2;\n#endif\n");
        assert!(out.contains("yes;") && !out.contains("no;") && !out.contains("no2;"));
    }

    #[test]
    fn names_with_macro_like_parts_are_untouched() {
        let out = pp("#define gpio 9\nreset-gpios = <&gpio 1 0>;\nfoo,gpio;\n");
        assert!(out.contains("reset-gpios = <&gpio 1 0>;"), "{out}");
        assert!(out.contains("foo,gpio;"), "{out}");
    }
}
