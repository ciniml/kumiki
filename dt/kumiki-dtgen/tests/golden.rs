//! Repository-level checks of the board descriptions.
//!
//! * `generated_code_is_up_to_date`: regenerating from the DTS must reproduce
//!   the checked-in `boards/kumiki-boards/src/esp32s3/generated.rs`.
//! * `dts_compiles_with_dtc`: the same DTS files compile with the reference
//!   `cpp` + `dtc` without warnings (skipped when the tools are missing).

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use kumiki_dtgen::{gen_esp32s3, init_order, validate, Bindings, Board};

const ESP32S3_BOARDS: &[&str] = &["boards/dts/m5stack/cores3.dts", "boards/dts/m5stack/atoms3.dts", "boards/dts/m5stack/stopwatch.dts"];
const ESP32S3_OUT: &str = "boards/kumiki-boards/src/esp32s3/generated.rs";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn includes(root: &Path) -> Vec<PathBuf> {
    vec![root.join("boards/dts"), root.join("boards/dts/include")]
}

fn boards(root: &Path) -> Vec<Board> {
    ESP32S3_BOARDS.iter().map(|b| Board::load(&root.join(b), &includes(root)).unwrap()).collect()
}

#[test]
fn boards_validate() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    for b in boards(&root) {
        let errs = validate(&b, &bindings);
        assert!(errs.is_empty(), "{}: {errs:#?}", b.source.display());
        init_order(&b).unwrap();
    }
}

#[test]
fn generated_code_is_up_to_date() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    let opt = gen_esp32s3::Options { module_path: "$crate::esp32s3::generated".into(), source_root: root.clone() };
    let code = gen_esp32s3::generate(&boards(&root), &bindings, &opt).unwrap();
    let on_disk = std::fs::read_to_string(root.join(ESP32S3_OUT)).unwrap();
    assert!(
        code == on_disk,
        "{ESP32S3_OUT} is stale; regenerate with:\n  cargo run -p kumiki-dtgen -- esp32s3 {ESP32S3_OUT} {}",
        ESP32S3_BOARDS.join(" ")
    );
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
    for b in ESP32S3_BOARDS {
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
