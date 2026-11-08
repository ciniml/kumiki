//! Repository-level checks of the board descriptions.
//!
//! * `generated_code_is_up_to_date`: regenerating from the DTS must reproduce
//!   the checked-in `boards/kumiki-boards/src/<family>/generated.rs`.
//! * `dts_compiles_with_dtc`: the same DTS files compile with the reference
//!   `cpp` + `dtc` without warnings (skipped when the tools are missing).

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use kumiki_dtgen::{
    gen_esp::{self, Backend},
    init_order, validate, Bindings, Board,
};

struct Family {
    name: &'static str,
    backend: Backend,
    boards: &'static [&'static str],
}

const FAMILIES: &[Family] = &[
    Family {
        name: "esp32s3",
        backend: Backend::Esp32s3,
        boards: &["boards/dts/m5stack/cores3.dts", "boards/dts/m5stack/atoms3.dts", "boards/dts/m5stack/stopwatch.dts"],
    },
    Family { name: "p4v1", backend: Backend::P4v1, boards: &["boards/dts/m5stack/tab5.dts"] },
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn includes(root: &Path) -> Vec<PathBuf> {
    vec![root.join("boards/dts"), root.join("boards/dts/include")]
}

fn boards(root: &Path, f: &Family) -> Vec<Board> {
    f.boards.iter().map(|b| Board::load(&root.join(b), &includes(root)).unwrap()).collect()
}

#[test]
fn boards_validate() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    for f in FAMILIES {
        for b in boards(&root, f) {
            let errs = validate(&b, &bindings);
            assert!(errs.is_empty(), "{}: {errs:#?}", b.source.display());
            init_order(&b).unwrap();
        }
    }
}

#[test]
fn generated_code_is_up_to_date() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    for f in FAMILIES {
        let opt = gen_esp::Options {
            backend: f.backend,
            module_path: format!("$crate::{}::generated", f.name),
            source_root: root.clone(),
        };
        let code = gen_esp::generate(&boards(&root, f), &bindings, &opt).unwrap();
        let out = format!("boards/kumiki-boards/src/{}/generated.rs", f.name);
        let on_disk = std::fs::read_to_string(root.join(&out)).unwrap();
        assert!(
            code == on_disk,
            "{out} is stale; regenerate with:\n  cargo run -p kumiki-dtgen -- {} {out} {}",
            f.name,
            f.boards.join(" ")
        );
    }
}

#[test]
fn dts_compiles_with_dtc() {
    let have = |t: &str| Command::new(t).arg("--version").output().is_ok();
    if !have("dtc") || !have("cpp") {
        eprintln!("skipping: dtc / cpp not installed");
        return;
    }
    let root = root();
    let out_dir = std::env::temp_dir().join(format!("kumiki-dtc-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    for b in FAMILIES.iter().flat_map(|f| f.boards.iter()) {
        let stem = Path::new(b).file_stem().unwrap().to_string_lossy().to_string();
        let pp = out_dir.join(format!("{stem}.pp.dts"));
        let cpp = Command::new("cpp")
            .args(["-nostdinc", "-undef", "-P", "-x", "assembler-with-cpp"])
            .arg("-I")
            .arg(root.join("boards/dts"))
            .arg("-I")
            .arg(root.join("boards/dts/include"))
            .arg(root.join(b))
            .arg("-o")
            .arg(&pp)
            .output()
            .unwrap();
        assert!(cpp.status.success(), "cpp {b}: {}", String::from_utf8_lossy(&cpp.stderr));
        let dtc = Command::new("dtc").args(["-I", "dts", "-O", "dtb", "-o"]).arg(out_dir.join(format!("{stem}.dtb"))).arg(&pp).output().unwrap();
        let stderr = String::from_utf8_lossy(&dtc.stderr);
        assert!(dtc.status.success() && stderr.trim().is_empty(), "dtc {b}:\n{stderr}");
    }
    let _ = std::fs::remove_dir_all(&out_dir);
}
