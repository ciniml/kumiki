//! Code generation for the ESP32-S3 family (esp-hal).
//!
//! Output (one file for all boards of the family):
//! * shared types (`Gpio`, `BoardId`, `DetectLog`);
//! * `detect()`: board detection generated from each board's `kumiki,detect`
//!   node, skipping candidates whose pins clash with embedded octal PSRAM;
//! * per board: constants for every device node (one module per node, shaped
//!   by its binding), the chip-pin bundle (`Pins` + `take_pins_*!` macro) and
//!   the dependency-ordered `INIT_ORDER`.

use std::fmt::Write as _;

use kumiki_dt::{Cell, Node, Tree, Value};

use crate::{chip_pins, gpios, init_order, is_present, Binding, Bindings, Board, Error, Gpio, PropType};

/// Generator options.
pub struct Options {
    /// Rust path of the generated module inside its crate, used by the
    /// exported macros (e.g. `$crate::esp32s3::generated`).
    pub module_path: String,
    /// Path prefix stripped from source paths in comments (keeps output stable).
    pub source_root: std::path::PathBuf,
}

fn ident_upper(s: &str) -> String {
    s.trim_start_matches("kumiki,").replace(['-', ',', '#', '.'], "_").to_uppercase()
}

fn ident_lower(s: &str) -> String {
    s.replace(['-', ',', '#', '.'], "_").to_lowercase()
}

fn variant_of(model: &str) -> String {
    model.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn feature_of(board: &Board) -> String {
    format!("board-{}", board.compatible().replace(',', "-"))
}

fn label(tree: &Tree, path: &str) -> String {
    let n = tree.node(path).unwrap();
    n.labels.first().cloned().unwrap_or_else(|| ident_lower(n.base_name()))
}

fn gpio_expr(g: &Gpio) -> String {
    match g {
        Gpio::Chip { pin, flags } => format!("Gpio::Chip {{ pin: {pin}, active_low: {} }}", flags & 1 != 0),
        Gpio::Expander { controller, pin, flags } => {
            format!("Gpio::Expander {{ controller: {controller:?}, pin: {pin}, active_low: {} }}", flags & 1 != 0)
        }
    }
}

fn ref_label(tree: &Tree, p: &kumiki_dt::Property) -> Option<String> {
    p.value.iter().find_map(|v| match v {
        Value::PathRef(r) => tree.resolve(r).map(|path| label(tree, &path)),
        Value::Cells(c) => c.iter().find_map(|c| match c {
            Cell::Ref(r) => tree.resolve(r).map(|path| label(tree, &path)),
            _ => None,
        }),
        _ => None,
    })
}

/// Emit the constants of one device node, shaped by its binding.
fn emit_node(out: &mut String, tree: &Tree, path: &str, n: &Node, b: &Binding) -> Result<(), Error> {
    let modname = label(tree, path);
    writeln!(out, "    /// `{}` ({}).", path, b.compatible).unwrap();
    writeln!(out, "    pub mod {modname} {{").unwrap();
    writeln!(out, "        #[allow(unused_imports)]\n        use super::super::Gpio;").unwrap();
    writeln!(out, "        pub const COMPATIBLE: &str = {:?};", n.compatible()[0]).unwrap();
    if n.status() == "fail-needs-probe" {
        writeln!(out, "        /// Second-source candidate: present only if probing finds it.").unwrap();
        writeln!(out, "        pub const NEEDS_PROBE: bool = true;").unwrap();
    }
    if let Some(g) = n.prop("kumiki,probe-group").and_then(|p| p.str()) {
        writeln!(out, "        pub const PROBE_GROUP: &str = {g:?};").unwrap();
    }
    if n.has("kumiki,optional") {
        writeln!(out, "        pub const OPTIONAL: bool = true;").unwrap();
    }
    for (name, spec) in &b.properties {
        let p = n.prop(name);
        let id = ident_upper(name);
        // Unit address of devices on an I²C bus.
        if name == "reg" && b.kumiki.bus.as_deref() == Some("i2c") {
            let v = p.and_then(|p| p.u32()).unwrap_or(0);
            writeln!(out, "        pub const ADDR: u8 = {v:#04x};").unwrap();
            continue;
        }
        if name == "reg" || name.starts_with('#') || name == "gpio-controller" || name == "ranges" {
            continue;
        }
        if let Some(d) = &spec.description {
            writeln!(out, "        /// {d}").unwrap();
        }
        let line = match (spec.ty, spec.required, p) {
            (PropType::Boolean, _, p) => format!("pub const {id}: bool = {};", p.is_some()),
            (PropType::Int, true, Some(p)) => format!("pub const {id}: u32 = {};", p.u32().unwrap_or(0)),
            (PropType::Int, false, Some(p)) => format!("pub const {id}: Option<u32> = Some({});", p.u32().unwrap_or(0)),
            (PropType::Int, false, None) => format!("pub const {id}: Option<u32> = None;"),
            (PropType::Sint, true, Some(p)) => format!("pub const {id}: i32 = {};", p.u32().unwrap_or(0) as i32),
            (PropType::Sint, false, Some(p)) => {
                format!("pub const {id}: Option<i32> = Some({});", p.u32().unwrap_or(0) as i32)
            }
            (PropType::Sint, false, None) => format!("pub const {id}: Option<i32> = None;"),
            (PropType::String, true, Some(p)) => format!("pub const {id}: &str = {:?};", p.str().unwrap_or("")),
            (PropType::String, false, Some(p)) => format!("pub const {id}: Option<&str> = Some({:?});", p.str().unwrap_or("")),
            (PropType::String, false, None) => format!("pub const {id}: Option<&str> = None;"),
            (PropType::StringArray, _, p) => {
                let v = p.map(|p| p.strs()).unwrap_or_default();
                format!("pub const {id}: &[&str] = &{v:?};")
            }
            (PropType::Array, _, p) => {
                let v = p.map(|p| p.u32s()).unwrap_or_default();
                format!("pub const {id}: &[u32] = &{v:?};")
            }
            (PropType::Uint8Array, _, p) => {
                let v = p.map(|p| p.bytes()).unwrap_or_default();
                format!("pub const {id}: &[u8] = &{v:?};")
            }
            (PropType::Phandle, true, Some(p)) => format!("pub const {id}: &str = {:?};", ref_label(tree, p).unwrap_or_default()),
            (PropType::Phandle, false, Some(p)) => {
                format!("pub const {id}: Option<&str> = Some({:?});", ref_label(tree, p).unwrap_or_default())
            }
            (PropType::Phandle, false, None) => format!("pub const {id}: Option<&str> = None;"),
            (PropType::Gpios, _, p) => {
                let list = match p {
                    Some(p) => gpios(tree, p)?,
                    None => Vec::new(),
                };
                let items: Vec<String> = list.iter().map(gpio_expr).collect();
                format!("pub const {id}: &[Gpio] = &[{}];", items.join(", "))
            }
            (PropType::PhandleArray, _, _) => continue,
            (_, true, None) => return Err(Error(format!("{path}: missing required {name}"))),
        };
        writeln!(out, "        {line}").unwrap();
    }
    // Child entries (gpio-keys).
    if !b.kumiki.child_props.is_empty() {
        let mut keys = Vec::new();
        for c in &n.children {
            let key = c.prop("kumiki,button").and_then(|p| p.str()).unwrap_or(c.base_name()).to_string();
            if let Some(p) = c.prop("gpios") {
                for g in gpios(tree, p)? {
                    keys.push(format!("({key:?}, {})", gpio_expr(&g)));
                }
            }
        }
        writeln!(out, "        /// (button, GPIO) entries.").unwrap();
        writeln!(out, "        pub const KEYS: &[(&str, Gpio)] = &[{}];", keys.join(", ")).unwrap();
    }
    writeln!(out, "    }}").unwrap();
    Ok(())
}

/// Detection code for one board (body of an `if candidate { … }`).
fn emit_detect(out: &mut String, board: &Board) -> Result<(), Error> {
    let tree = &board.tree;
    let Some(det) = tree.root.children.iter().find(|c| c.compatible().contains(&"kumiki,board-detect")) else {
        return Err(Error(format!("{}: no kumiki,detect node", board.source.display())));
    };
    let model = board.model();
    for probe in &det.children {
        let comp = probe.compatible();
        let pname = probe.base_name();
        let target_path = |prop: &str| -> Result<String, Error> {
            let p = probe.prop(prop).ok_or_else(|| Error(format!("{pname}: missing {prop}")))?;
            let r = match p.value.first() {
                Some(Value::Cells(c)) => match c.first() {
                    Some(Cell::Ref(r)) => r.clone(),
                    _ => String::new(),
                },
                Some(Value::PathRef(r)) => r.clone(),
                _ => String::new(),
            };
            tree.resolve(&r).ok_or_else(|| Error(format!("{pname}: unresolved {prop}")))
        };
        let chip_pin = |n: &Node, prop: &str| -> Result<u32, Error> {
            let p = n.prop(prop).ok_or_else(|| Error(format!("{pname}: target lacks {prop}")))?;
            match gpios(tree, p)?.first() {
                Some(Gpio::Chip { pin, .. }) => Ok(*pin),
                _ => Err(Error(format!("{pname}: {prop} must be a chip GPIO for probing"))),
            }
        };
        writeln!(out, "        // probe `{pname}` ({})", comp.join(", ")).unwrap();
        if comp.contains(&"kumiki,probe-i2c-reg") || comp.contains(&"kumiki,probe-i2c-ack") {
            let dev = target_path("device")?;
            let bus_path = Tree::parent_path(&dev).unwrap();
            let bus = tree.node(&bus_path).unwrap();
            let addr = tree.node(&dev).unwrap().prop("reg").and_then(|p| p.u32()).unwrap_or(0);
            let reg = probe.prop("register").and_then(|p| p.u32()).unwrap_or(0);
            let inst = bus.prop("kumiki,instance").and_then(|p| p.str()).unwrap_or("I2C0");
            let (sda, scl) = (chip_pin(bus, "sda-gpios")?, chip_pin(bus, "scl-gpios")?);
            writeln!(out, "        if ok {{").unwrap();
            writeln!(out, "            super::probe::i2c_bus_recover(p.GPIO{scl}.reborrow(), p.GPIO{sda}.reborrow());").unwrap();
            writeln!(
                out,
                "            let v = super::probe::i2c_read_reg(p.{inst}.reborrow(), p.GPIO{sda}.reborrow(), p.GPIO{scl}.reborrow(), {addr:#04x}, {reg:#04x});"
            )
            .unwrap();
            writeln!(out, "            log.push({model:?}, {pname:?}, v.map(|b| [b, 0, 0]), 1);").unwrap();
            if comp.contains(&"kumiki,probe-i2c-reg") {
                let expect = probe.prop("expect").and_then(|p| p.u32()).unwrap_or(0);
                writeln!(out, "            ok = v == Some({expect:#04x});").unwrap();
            } else {
                writeln!(out, "            ok = v.is_some();").unwrap();
            }
            writeln!(out, "        }}").unwrap();
        } else if comp.contains(&"kumiki,probe-dcs-id-3wire") {
            let disp_path = target_path("display")?;
            let disp = tree.node(&disp_path).unwrap();
            let spi = tree.node(&Tree::parent_path(&disp_path).unwrap()).unwrap();
            let (sck, sda, cs) = (chip_pin(spi, "sck-gpios")?, chip_pin(spi, "mosi-gpios")?, chip_pin(spi, "cs-gpios")?);
            let (dc, rst) = (chip_pin(disp, "dc-gpios")?, chip_pin(disp, "reset-gpios")?);
            let expect = probe.prop("expect").map(|p| p.bytes()).unwrap_or_default();
            if expect.len() != 3 {
                return Err(Error(format!("{pname}: expect must be 3 bytes")));
            }
            writeln!(out, "        if ok {{").unwrap();
            writeln!(
                out,
                "            let id = super::probe::read_dcs_id_3wire(p.GPIO{sck}.reborrow(), p.GPIO{sda}.reborrow(), p.GPIO{cs}.reborrow(), p.GPIO{dc}.reborrow(), p.GPIO{rst}.reborrow());"
            )
            .unwrap();
            writeln!(out, "            log.push({model:?}, {pname:?}, Some(id), 3);").unwrap();
            writeln!(out, "            ok = id == {expect:?};").unwrap();
            writeln!(out, "        }}").unwrap();
        } else {
            return Err(Error(format!("{pname}: unknown probe {comp:?}")));
        }
    }
    Ok(())
}

/// Generate the family module.
pub fn generate(boards: &[Board], bindings: &Bindings, opt: &Options) -> Result<String, Error> {
    let mut problems = Vec::new();
    for b in boards {
        for e in crate::validate(b, bindings) {
            problems.push(format!("{}: {e}", b.source.display()));
        }
    }
    if !problems.is_empty() {
        return Err(Error(problems.join("\n")));
    }
    let rel = |p: &std::path::Path| p.strip_prefix(&opt.source_root).unwrap_or(p).display().to_string();
    let mut out = String::new();
    writeln!(out, "// @generated by kumiki-dtgen — do not edit by hand.").unwrap();
    for b in boards {
        writeln!(out, "// source: {}", rel(&b.source)).unwrap();
    }
    out.push_str(
        r#"#![allow(dead_code, clippy::all, missing_docs)]

use esp_hal::peripherals::Peripherals;

/// A GPIO as described in the device tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpio {
    /// A chip GPIO.
    Chip { pin: u8, active_low: bool },
    /// A pin of an IO-expander node (by label).
    Expander { controller: &'static str, pin: u8, active_low: bool },
}

/// One probe result, kept for diagnostics when no board matched.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProbeRecord {
    pub board: &'static str,
    pub probe: &'static str,
    /// `None`: no answer (NACK).
    pub value: Option<[u8; 3]>,
    pub len: u8,
}

/// What detection saw.
#[derive(Clone, Copy, Debug)]
pub struct DetectLog {
    pub octal_psram: bool,
    pub psram: bool,
    pub records: [ProbeRecord; 8],
    pub count: usize,
}

impl DetectLog {
    fn push(&mut self, board: &'static str, probe: &'static str, value: Option<[u8; 3]>, len: u8) {
        if self.count < self.records.len() {
            self.records[self.count] = ProbeRecord { board, probe, value, len };
            self.count += 1;
        }
    }
}

"#,
    );
    // Board ids.
    writeln!(out, "/// Boards of this family, in detection order.").unwrap();
    writeln!(out, "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub enum BoardId {{").unwrap();
    for b in boards {
        writeln!(out, "    /// {} (`{}`).\n    {},", b.model(), b.compatible(), variant_of(b.model())).unwrap();
    }
    writeln!(out, "}}\n\nimpl BoardId {{\n    /// Board name (`model`).\n    pub const fn name(self) -> &'static str {{\n        match self {{").unwrap();
    for b in boards {
        writeln!(out, "            BoardId::{} => {:?},", variant_of(b.model()), b.model()).unwrap();
    }
    writeln!(out, "        }}\n    }}\n\n    /// Root `compatible`.\n    pub const fn compatible(self) -> &'static str {{\n        match self {{").unwrap();
    for b in boards {
        writeln!(out, "            BoardId::{} => {:?},", variant_of(b.model()), b.compatible()).unwrap();
    }
    writeln!(out, "        }}\n    }}\n}}\n").unwrap();
    let feats: Vec<String> = boards.iter().map(|b| format!("feature = {:?}", feature_of(b))).collect();
    writeln!(out, "/// Some `board-*` feature pins the candidate set.\nconst ANY_PINNED: bool = cfg!(any({}));\n", feats.join(", ")).unwrap();

    // detect()
    out.push_str(
        "/// Detect the board. `octal_psram`: the chip embeds octal PSRAM (GPIO33..37 are\n\
         /// taken); `psram`: any PSRAM. Peripherals are only borrowed.\n\
         pub fn detect(p: &mut Peripherals, octal_psram: bool, psram: bool) -> Result<BoardId, DetectLog> {\n\
         \x20   let mut log = DetectLog { octal_psram, psram, records: [ProbeRecord::default(); 8], count: 0 };\n",
    );
    for b in boards {
        let m = &b.module;
        writeln!(
            out,
            "    if {m}::CANDIDATE && !(octal_psram && {m}::USES_OCTAL_PSRAM_PINS) && (psram || !{m}::NEEDS_PSRAM) {{\n        let mut ok = true;"
        )
        .unwrap();
        emit_detect(&mut out, b)?;
        writeln!(out, "        if ok {{\n            return Ok(BoardId::{});\n        }}\n    }}", variant_of(b.model())).unwrap();
    }
    writeln!(out, "    Err(log)\n}}\n").unwrap();

    // Pin bundles and macros.
    for b in boards {
        let pins = chip_pins(b);
        writeln!(out, "/// Move the chip GPIOs used by {} out of `Peripherals`.", b.model()).unwrap();
        writeln!(out, "macro_rules! take_pins_{} {{\n    ($p:ident) => {{\n        {}::{}::Pins {{", b.module, opt.module_path, b.module).unwrap();
        for (field, pin) in &pins {
            writeln!(out, "            {field}: $p.GPIO{pin},").unwrap();
        }
        writeln!(out, "        }}\n    }};\n}}\n").unwrap();
    }

    // Per-board modules.
    for b in boards {
        let tree = &b.tree;
        let pins = chip_pins(b);
        let octal: Vec<u32> = {
            let mut v = Vec::new();
            tree.root.walk("/", &mut |_, n| {
                if let Some(p) = n.prop("kumiki,octal-psram-pins") {
                    v = p.u32s();
                }
            });
            v
        };
        let uses_octal = pins.iter().any(|(_, pin)| octal.contains(pin));
        let mut needs_psram = false;
        tree.root.walk("/", &mut |_, n| {
            if is_present(n) && n.prop("kumiki,frame-buffer").and_then(|p| p.str()) == Some("psram") {
                needs_psram = true;
            }
        });
        let order = init_order(b)?;
        writeln!(out, "/// {} (`{}`), from `{}`.", b.model(), b.compatible(), rel(&b.source)).unwrap();
        writeln!(out, "pub mod {} {{", b.module).unwrap();
        writeln!(out, "    pub const NAME: &str = {:?};", b.model()).unwrap();
        writeln!(out, "    pub const COMPATIBLE: &str = {:?};", b.compatible()).unwrap();
        writeln!(out, "    /// Detection candidate (no `board-*` feature, or `{}`).", feature_of(b)).unwrap();
        writeln!(out, "    pub const CANDIDATE: bool = !super::ANY_PINNED || cfg!(feature = {:?});", feature_of(b)).unwrap();
        writeln!(out, "    /// Uses GPIOs owned by embedded octal PSRAM: never probed on such chips.").unwrap();
        writeln!(out, "    pub const USES_OCTAL_PSRAM_PINS: bool = {uses_octal};").unwrap();
        writeln!(out, "    /// Needs PSRAM (a frame buffer lives there).").unwrap();
        writeln!(out, "    pub const NEEDS_PSRAM: bool = {needs_psram};").unwrap();
        let order_s: Vec<String> = order.iter().map(|s| format!("{s:?}")).collect();
        writeln!(out, "    /// Device bring-up order (dependency graph of the device tree).").unwrap();
        writeln!(out, "    pub const INIT_ORDER: &[&str] = &[{}];", order_s.join(", ")).unwrap();
        writeln!(out, "\n    /// Chip GPIOs used by the board (see `take_pins_{}!`).", b.module).unwrap();
        writeln!(out, "    pub struct Pins {{").unwrap();
        for (field, pin) in &pins {
            writeln!(out, "        pub {field}: esp_hal::peripherals::GPIO{pin}<'static>,").unwrap();
        }
        writeln!(out, "    }}\n").unwrap();
        let mut res = Ok(());
        tree.root.walk("/", &mut |path, n| {
            if res.is_err() || !is_present(n) || n.compatible().is_empty() || path == "/" {
                return;
            }
            let c = n.compatible();
            if c.iter().any(|c| c.ends_with("-gpio") || *c == "simple-bus" || c.starts_with("kumiki,probe") || *c == "kumiki,board-detect") {
                return;
            }
            let binding = bindings.for_node(n).expect("validated");
            res = emit_node(&mut out, tree, path, n, binding);
        });
        res?;
        writeln!(out, "}}\n").unwrap();
    }
    // Trim the final blank line for a stable file ending.
    while out.ends_with("\n\n") {
        out.pop();
    }
    Ok(out)
}
