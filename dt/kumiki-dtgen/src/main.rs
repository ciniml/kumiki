//! `kumiki-dtgen` CLI.
//!
//! ```text
//! kumiki-dtgen esp32s3 <out.rs> <board.dts>...      generate for ESP32-S3 / esp-hal (paths relative to the repository root)
//! kumiki-dtgen p4v1 <out.rs> <board.dts>...         generate for ESP32-P4 rev v1 / kumiki-p4v1-hal
//! kumiki-dtgen check <board.dts>...                 validate only
//! kumiki-dtgen order <board.dts>                    print the init order
//! ```
//! Include directories: `boards/dts` and `boards/dts/include`; bindings: `boards/bindings`.

use std::path::{Path, PathBuf};

use kumiki_dtgen::{
    gen_esp::{self, Backend},
    init_order, validate, Bindings, Board,
};

fn repo_root() -> PathBuf {
    // dt/kumiki-dtgen → repository root
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = repo_root();
    let inc = vec![root.join("boards/dts"), root.join("boards/dts/include")];
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap_or_else(|e| fail(&e.to_string()));
    let load = |p: &String| Board::load(&root.join(p), &inc).unwrap_or_else(|e| fail(&e.to_string()));
    match args.first().map(String::as_str) {
        Some(family @ ("esp32s3" | "p4v1")) if args.len() >= 3 => {
            let boards: Vec<Board> = args[2..].iter().map(load).collect();
            let backend = if family == "p4v1" { Backend::P4v1 } else { Backend::Esp32s3 };
            let opt = gen_esp::Options { backend, module_path: format!("$crate::{family}::generated"), source_root: root.clone() };
            let code = gen_esp::generate(&boards, &bindings, &opt).unwrap_or_else(|e| fail(&e.to_string()));
            std::fs::write(root.join(&args[1]), code).unwrap_or_else(|e| fail(&e.to_string()));
            eprintln!("wrote {}", args[1]);
        }
        Some("check") if args.len() >= 2 => {
            let mut bad = false;
            for a in &args[1..] {
                let b = load(a);
                for e in validate(&b, &bindings) {
                    eprintln!("{a}: {e}");
                    bad = true;
                }
            }
            if bad {
                std::process::exit(1);
            }
            eprintln!("ok");
        }
        Some("order") if args.len() == 2 => {
            let b = load(&args[1]);
            println!("{}", init_order(&b).unwrap_or_else(|e| fail(&e.to_string())).join(" -> "));
        }
        _ => fail("usage: kumiki-dtgen esp32s3|p4v1 <out.rs> <board.dts>... | check <board.dts>... | order <board.dts>"),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
