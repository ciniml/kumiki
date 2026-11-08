//! `kumiki-dtgen` CLI.
//!
//! ```text
//! kumiki-dtgen esp32s3 <out.rs> <board.dts>...      generate for ESP32-S3 / esp-hal (paths relative to the repository root)
//! kumiki-dtgen p4v1 <out.rs> <board.dts>...         generate for ESP32-P4 rev v1 / kumiki-p4v1-hal
//! kumiki-dtgen soc-dtsi esp32s3 <out.dtsi>          SoC layer from esp-metadata-generated + PAC (versions from firmware/Cargo.lock)
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
        Some("soc-dtsi") if args.len() == 3 => {
            let chip = args[1].as_str();
            let code = soc_dtsi_for(&root, chip).unwrap_or_else(|e| fail(&e));
            std::fs::write(root.join(&args[2]), code).unwrap_or_else(|e| fail(&e.to_string()));
            eprintln!("wrote {}", args[2]);
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

/// Version of `name` in `firmware/Cargo.lock` (what the firmware builds with).
fn locked_version(root: &Path, name: &str) -> Result<String, String> {
    let lock = std::fs::read_to_string(root.join("firmware/Cargo.lock")).map_err(|e| format!("firmware/Cargo.lock: {e}"))?;
    let pat = format!("name = \"{name}\"\nversion = \"");
    let i = lock.find(&pat).ok_or_else(|| format!("{name} is not in firmware/Cargo.lock (build the firmware first)"))?;
    let rest = &lock[i + pat.len()..];
    Ok(rest[..rest.find('"').unwrap_or(0)].to_string())
}

/// Source directory of crate `name-version` in the cargo registry.
fn registry_dir(name: &str, version: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .ok_or("no CARGO_HOME / HOME")?;
    let src = home.join("registry/src");
    for idx in std::fs::read_dir(&src).map_err(|e| format!("{}: {e}", src.display()))?.flatten() {
        let d = idx.path().join(format!("{name}-{version}"));
        if d.is_dir() {
            return Ok(d);
        }
    }
    Err(format!("{name}-{version} not found under {} (cargo fetch in firmware/)", src.display()))
}

/// The SoC `.dtsi` for `chip`, from the esp-metadata-generated and PAC
/// versions locked by the firmware.
fn soc_dtsi_for(root: &Path, chip: &str) -> Result<String, String> {
    let (meta_v, pac_v) = (locked_version(root, "esp-metadata-generated")?, locked_version(root, chip)?);
    let meta_file = format!("src/_generated_{chip}.rs");
    let meta = std::fs::read_to_string(registry_dir("esp-metadata-generated", &meta_v)?.join(&meta_file)).map_err(|e| e.to_string())?;
    let pac = std::fs::read_to_string(registry_dir(chip, &pac_v)?.join("src/lib.rs")).map_err(|e| e.to_string())?;
    let info = kumiki_dtgen::soc_dtsi::parse_metadata(&meta).map_err(|e| e.to_string())?;
    let sources = vec![
        format!("esp-metadata-generated {meta_v} ({meta_file}): GPIOs, I2C / SPI masters, octal PSRAM pins"),
        format!("{chip} {pac_v} (PAC src/lib.rs): base addresses"),
    ];
    kumiki_dtgen::soc_dtsi::generate(chip, &info, &pac, &sources).map_err(|e| e.to_string())
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
