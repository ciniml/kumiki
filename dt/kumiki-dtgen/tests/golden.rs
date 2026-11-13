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
        boards: &[
            "boards/dts/m5stack/cores3-stackchan.dts",
            "boards/dts/m5stack/cores3.dts",
            "boards/dts/m5stack/atoms3.dts",
            "boards/dts/m5stack/stopwatch.dts",
        ],
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

/// The text of `pub mod <module>` in generated code.
fn board_module<'a>(code: &'a str, module: &str) -> &'a str {
    let start = code.find(&format!("\npub mod {module} {{")).unwrap();
    let rest = &code[start + 1..];
    &rest[..rest.find("\n}\n").unwrap()]
}

#[test]
fn unused_resources_are_handed_to_the_application() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    let f = &FAMILIES[0];
    let opt = gen_esp::Options { backend: f.backend, module_path: "$crate::esp32s3::generated".into(), source_root: root.clone() };
    let code = gen_esp::generate(&boards(&root, f), &bindings, &opt).unwrap();
    // Family struct: SPI flash pins are never listed; octal PSRAM pins can be withheld.
    assert!(code.contains("pub gpio6: Option<esp_hal::peripherals::GPIO6<'static>>,"));
    assert!(!code.contains("pub gpio26:"));
    assert!(code.contains("self.gpio33 = None;"));
    let cores3 = board_module(&code, "cores3");
    // Stack-chan base servo UART pins; the internal bus pins are taken.
    for want in ["gpio6: Some(p.GPIO6)", "gpio7: Some(p.GPIO7)", "gpio12: None", "gpio11: None", "gpio35: None"] {
        assert!(cores3.contains(want), "cores3: {want}");
    }
    // CoreS3 uses I2C0, so I2C1 (Port A) is free; its bus is handed over.
    for want in ["i2c0: None", "i2c1: Some(p.I2C1)", "spi2: None", "i2c: Some(i2c0)"] {
        assert!(cores3.contains(want), "cores3: {want}");
    }
    let atoms3 = board_module(&code, "atoms3");
    for want in ["i2c0: Some(p.I2C0)", "i2c1: None", "gpio38: None", "i2c: Some(i2c1)", "i2s1: Some(p.I2S1)"] {
        assert!(atoms3.contains(want), "atoms3: {want}");
    }
    // A board uses UART1, so it is in `Free` (an `Option`), not in `Spare`.
    assert!(code.contains("pub uart1: Option<esp_hal::peripherals::UART1<'static>>,"));
    assert!(code.contains("pub uart0: esp_hal::peripherals::UART0<'static>,"));
    assert!(cores3.contains("uart1: Some(p.UART1)") && cores3.contains("extras: super::Extras::None"));
}

#[test]
fn buffers_come_from_the_caller_sized_per_board() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    let f = &FAMILIES[0];
    let opt = gen_esp::Options { backend: f.backend, module_path: "$crate::esp32s3::generated".into(), source_root: root.clone() };
    let code = gen_esp::generate(&boards(&root, f), &bindings, &opt).unwrap();
    // No DMA buffer statics in the generated bring-up.
    assert!(!code.contains("dma_tx_stream_buffer!") && !code.contains("dma_rx_stream_buffer!"));
    assert!(board_module(&code, "cores3").contains(
        "pub const BUFFERS: BufferNeeds = BufferNeeds { display: 16384, i2s_tx: (16000, 4000), i2s_rx: (16000, 1000) };"
    ));
    let atoms3 = board_module(&code, "atoms3");
    assert!(atoms3.contains("pub const BUFFERS: BufferNeeds = BufferNeeds { display: 16384, i2s_tx: (0, 0), i2s_rx: (0, 0) };"));
    assert!(atoms3.contains("let (buf_a, buf_b) = bufs.display.take().ok_or(InitError::MissingBuffer(\"display\"))?;"));
    // Only detection candidates are linked in.
    assert!(code.contains("BoardId::M5StackAtomS3 if atoms3::CANDIDATE => atoms3::init(p, bufs),"));
    assert!(code.contains("let m = if atoms3::CANDIDATE { m.max(atoms3::BUFFERS) } else { m };"));
}

#[test]
fn psram_mode_comes_from_the_dts_and_the_rest_goes_to_the_application() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    let f = &FAMILIES[0];
    let opt = gen_esp::Options { backend: f.backend, module_path: "$crate::esp32s3::generated".into(), source_root: root.clone() };
    let code = gen_esp::generate(&boards(&root, f), &bindings, &opt).unwrap();
    let sw = board_module(&code, "stopwatch");
    for want in [
        "pub const MODE: &str = \"octal\";",
        "let mut psram = bind::map_psram(p.PSRAM, psram::MODE);",
        // The frame buffer is carved from it; the rest is handed over.
        "let fb = bind::carve(&mut psram, w as usize * h as usize * 2)",
        "psram: psram,",
        "pub const NEEDS_PSRAM: bool = true;",
        "pub const INIT_ORDER: &[&str] = &[\"psram\",",
    ] {
        assert!(sw.contains(want), "stopwatch: {want}");
    }
    let cores3 = board_module(&code, "cores3");
    assert!(cores3.contains("pub const MODE: &str = \"quad\";") && cores3.contains("psram: None,\n"));
    // AtomS3 declares no PSRAM: the peripheral stays free.
    let atoms3 = board_module(&code, "atoms3");
    assert!(atoms3.contains("psram: Some(p.PSRAM),") && !atoms3.contains("map_psram"));
}

#[test]
fn psram_mode_must_be_one_the_chip_supports() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    let mut b = Board::load(&root.join("boards/dts/m5stack/stopwatch.dts"), &includes(&root)).unwrap();
    let node = b.tree.node_mut("/psram").unwrap();
    let mode = node.props.iter_mut().find(|p| p.name == "kumiki,mode").unwrap();
    mode.value = vec![kumiki_dt::Value::Str("hex".into())];
    let errs = validate(&b, &bindings);
    assert!(errs.iter().any(|e| e.contains("kumiki,mode \"hex\" is not one of")), "{errs:#?}");
}

#[test]
fn stackchan_base_is_an_extra_set_of_the_cores3_variant() {
    let root = root();
    let bindings = Bindings::load_dir(&root.join("boards/bindings")).unwrap();
    let f = &FAMILIES[0];
    let opt = gen_esp::Options { backend: f.backend, module_path: "$crate::esp32s3::generated".into(), source_root: root.clone() };
    let code = gen_esp::generate(&boards(&root, f), &bindings, &opt).unwrap();
    // Detected first; the PY32 is asked until its boot time has passed.
    assert!(code.find("return Ok(BoardId::M5StackCoreS3Stackchan)").unwrap() < code.find("return Ok(BoardId::M5StackCoreS3)").unwrap());
    assert!(code.contains("if v.is_some_and(|b| b != 0x00 && b != 0xff) || esp_hal::time::Instant::now().duration_since_epoch().as_millis() >= 1200 {"));
    let m = board_module(&code, "cores3_stackchan");
    for want in [
        // The servo bus takes UART1 and GPIO6/7.
        "uart1: None",
        "gpio6: None",
        "gpio7: None",
        ".with_tx(pins.uart1_tx)",
        // The servo supply is a rail switched by PY32 line 0, off after bring-up.
        "pub static SERVO_POWER: kumiki_class::RailCount = kumiki_class::RailCount::new(false);",
        "pub servo_power: kumiki_class::Rail<kumiki_class::GpioSwitch<kumiki_py32::Py32Pin<SharedI2c>, Delay>>,",
        "(0, chips::LineInit::OutputLow)",
        "pub battery: Option<kumiki_ina226::Ina226<SharedI2c>>,",
        "pub uart1: esp_hal::uart::Uart<'static, esp_hal::Blocking>,",
        "extras: super::Extras::M5StackCoreS3Stackchan(Extras { base_io: base_io, servo_power: servo_power, battery: battery, uart1: uart1 })",
        // Servo ids and poses as constants.
        "pub const ID: u8 = 1;",
        "pub const ZERO: Option<u32> = Some(620);",
    ] {
        assert!(m.contains(want), "cores3_stackchan: {want}");
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
