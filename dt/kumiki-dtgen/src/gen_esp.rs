//! Code generation for Espressif chip families: ESP32-S3 on esp-hal
//! ([`Backend::Esp32s3`]) and ESP32-P4 rev v1 on kumiki-p4v1-hal
//! ([`Backend::P4v1`]). The two differ only in the HAL calls the generator
//! itself writes (pin bundles, probes, the `init` signature) and in which
//! binding template is used (`init-for.<backend>`, else `init`).
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

/// Target chip family / HAL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// ESP32-S3, esp-hal.
    Esp32s3,
    /// ESP32-P4 chip revision v1.x, kumiki-p4v1-hal.
    P4v1,
}

impl Backend {
    /// Key of this backend in a binding's `init-for`.
    pub fn key(self) -> &'static str {
        match self {
            Backend::Esp32s3 => "esp32s3",
            Backend::P4v1 => "p4v1",
        }
    }

    fn prelude(self) -> &'static str {
        match self {
            Backend::Esp32s3 => PRELUDE,
            Backend::P4v1 => PRELUDE_P4V1,
        }
    }

    /// `$p.<pin>` in the `take_pins_*!` macro, and the field type.
    fn pin_take(self, pin: u32) -> String {
        match self {
            Backend::Esp32s3 => format!("$p.GPIO{pin}"),
            Backend::P4v1 => format!("$p.pins.gpio{pin}"),
        }
    }
    fn pin_type(self, pin: u32) -> String {
        match self {
            Backend::Esp32s3 => format!("esp_hal::peripherals::GPIO{pin}<'static>"),
            Backend::P4v1 => format!("kumiki_p4v1_hal::gpio::Pin<{pin}>"),
        }
    }
    fn output_high(self, pin: &str) -> String {
        match self {
            Backend::Esp32s3 => format!("Output::new({pin}, Level::High, OutputConfig::default())"),
            Backend::P4v1 => format!("Output::new({pin}, true)"),
        }
    }
    fn input_pullup(self, pin: &str) -> String {
        match self {
            Backend::Esp32s3 => format!("Input::new({pin}, InputConfig::default().with_pull(Pull::Up))"),
            Backend::P4v1 => format!("Input::new({pin}, Pull::Up)"),
        }
    }
    fn peripherals(self) -> &'static str {
        match self {
            Backend::Esp32s3 => "esp_hal::peripherals::Peripherals",
            Backend::P4v1 => "kumiki_p4v1_hal::Peripherals",
        }
    }
    /// Extra arguments of the generated `init` (after `p`): parameters, call.
    fn init_args(self) -> (&'static str, &'static str) {
        match self {
            Backend::Esp32s3 => (", buf_a: DmaTxBuf, buf_b: DmaTxBuf", ", buf_a, buf_b"),
            Backend::P4v1 => (", psram: (usize, usize)", ", psram"),
        }
    }
    fn init_args_dispatch(self) -> &'static str {
        match self {
            Backend::Esp32s3 => ", buf_a: prelude::DmaTxBuf, buf_b: prelude::DmaTxBuf",
            Backend::P4v1 => ", psram: (usize, usize)",
        }
    }
    fn delay_new(self) -> &'static str {
        match self {
            Backend::Esp32s3 => "Delay::new()",
            Backend::P4v1 => "Delay",
        }
    }
}

/// Generator options.
pub struct Options {
    /// Target family / HAL.
    pub backend: Backend,
    /// Rust path of the generated module inside its crate, used by the
    /// exported macros (e.g. `$crate::esp32s3::generated`).
    pub module_path: String,
    /// Path prefix stripped from source paths in comments (keeps output stable).
    pub source_root: std::path::PathBuf,
}

/// Items the generated bring-up code and the binding templates use. The
/// family module (`super` of the generated module) provides the board-side
/// types (`Devices`, `Any*`, `chips`, `bind`, `probe`, …).
const PRELUDE: &str = r#"mod prelude {
    pub use core::cell::RefCell;
    pub use embedded_graphics_core::{draw_target::DrawTarget, geometry::Size, pixelcolor::{Rgb565, RgbColor}};
    pub use embedded_hal::delay::DelayNs as _;
    pub use embedded_hal_bus::i2c::RefCellDevice;
    pub use esp_hal::{
        delay::Delay,
        dma::DmaTxBuf,
        gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
        i2c::master::{Config as I2cConfig, I2c},
        i2s::master::{Channels, DataFormat, I2s, TdmConfig},
        peripherals::Peripherals,
        spi::{Mode, master::{Config as SpiConfig, Spi}},
        time::Rate,
    };
    pub use kumiki_gfx::Display;
    pub use kumiki_gfx_bus_esp::{EspQspiDmaBus, EspSpiDmaBus, SpiHost};
    pub use kumiki_gfx_panels::{DcsPanel, Model, NoPin};
    pub use kumiki_mpu6886::Mpu6886;
    pub use static_cell::StaticCell;
    pub use super::super::{
        audio, bind, chips, probe, AnyBacklight, AnyDisplay, AnyImu, AnyMicrophone, AnyPower, AnyRtc, AnySpeaker, AnyTouch, ButtonInputs,
        Devices, I2cBus, InitError,
        Spare,
    };
}
"#;

/// [`PRELUDE`] for the ESP32-P4 rev v1 family.
const PRELUDE_P4V1: &str = r#"mod prelude {
    #![allow(unused_imports)]
    pub use core::cell::RefCell;
    pub use embedded_hal::delay::DelayNs as _;
    pub use embedded_hal_bus::i2c::RefCellDevice;
    pub use kumiki_gfx::FbDisplay;
    pub use kumiki_p4v1_hal::{
        Peripherals,
        dsi::Dsi,
        fb::DsiPanel,
        gpio::{Input, Output, Pull},
        i2c::I2c,
        time::Delay,
    };
    pub use static_cell::StaticCell;
    pub use super::super::{
        bind, chips, probe, AnyBacklight, AnyDisplay, AnyImu, AnyMicrophone, AnyPower, AnyRtc, AnySpeaker, AnyTouch, ButtonInputs, Devices,
        I2cBus, InitError,
        Spare,
    };
}
"#;

// ---- template expansion ---------------------------------------------------------

struct Ctx<'a> {
    board: &'a Board,
    bindings: &'a Bindings,
    path: String,
    backend: Backend,
}

/// The template `backend` uses for `b`: `init-for.<key>`, else `init`.
fn init_template(b: &Binding, backend: Backend) -> Option<&str> {
    b.kumiki.init_for.get(backend.key()).or(b.kumiki.init.as_ref()).map(String::as_str)
}

impl Ctx<'_> {
    fn tree(&self) -> &Tree {
        &self.board.tree
    }
    fn node(&self, path: &str) -> &Node {
        self.tree().node(path).unwrap()
    }
    fn err(&self, msg: impl std::fmt::Display) -> Error {
        Error(format!("{}: {}: {msg}", self.board.source.display(), self.path))
    }

    /// Resolve `parent.` / `@PROP.` prefixes: returns (node path, rest).
    fn target<'s>(&self, mut spec: &'s str) -> Result<(String, &'s str), Error> {
        let mut path = self.path.clone();
        loop {
            if let Some(rest) = spec.strip_prefix("parent.") {
                path = Tree::parent_path(&path).ok_or_else(|| self.err("no parent"))?;
                spec = rest;
            } else if let Some(rest) = spec.strip_prefix('@') {
                let (prop, rest) = rest.split_once('.').ok_or_else(|| self.err(format!("bad reference spec @{rest}")))?;
                let p = self.node(&path).prop(prop).ok_or_else(|| self.err(format!("missing {prop}")))?;
                path = phandle_target(self.tree(), p).ok_or_else(|| self.err(format!("{prop}: unresolved phandle")))?;
                spec = rest;
            } else {
                return Ok((path, spec));
            }
        }
    }

    fn pin(&self, arg: &str) -> Result<String, Error> {
        let (path, rest) = self.target(arg)?;
        let (prop, index) = match rest.split_once('[') {
            Some((p, i)) => (p, i.trim_end_matches(']').parse::<usize>().map_err(|_| self.err("bad pin index"))?),
            None => (rest, 0),
        };
        let p = self.node(&path).prop(prop).ok_or_else(|| self.err(format!("{path}: missing {prop}")))?;
        let list = gpios(self.tree(), p)?;
        match list.get(index) {
            Some(Gpio::Chip { .. }) => Ok(format!("pins.{}", crate::pin_field(self.tree(), &path, prop, index, list.len()))),
            Some(Gpio::Expander { .. }) => Err(self.err(format!("{prop}[{index}] is an expander line, not a chip GPIO"))),
            None => Err(self.err(format!("{prop} has no entry {index}"))),
        }
    }

    /// Variable of the I²C bus the device owning `path` sits on.
    fn bus_of(&self, path: &str) -> Result<String, Error> {
        let mut cur = path.to_string();
        while let Some(parent) = Tree::parent_path(&cur) {
            let pn = self.node(&parent);
            if self.bindings.for_node(pn).and_then(|b| b.kumiki.controller.as_deref()) == Some("i2c") {
                return Ok(label(self.tree(), &parent));
            }
            cur = parent;
        }
        Err(self.err(format!("{path} is not on an I²C bus")))
    }

    fn expand(&self, tpl: &str) -> Result<String, Error> {
        let mut out = String::new();
        let mut rest = tpl;
        while let Some(i) = rest.find("${") {
            out.push_str(&rest[..i]);
            let end = rest[i..].find('}').ok_or_else(|| self.err("unterminated ${"))? + i;
            let ph = &rest[i + 2..end];
            out.push_str(&self.placeholder(ph)?);
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn placeholder(&self, ph: &str) -> Result<String, Error> {
        let (kind, arg) = ph.split_once(':').unwrap_or((ph, ""));
        let tree = self.tree();
        let me = self.node(&self.path);
        let my_label = label(tree, &self.path);
        let binding = self.bindings.for_node(me).ok_or_else(|| self.err("no binding"))?;
        Ok(match kind {
            "label" => my_label,
            "const" => {
                let (path, prop) = self.target(arg)?;
                format!("{}::{}", label(tree, &path), ident_upper(prop))
            }
            "instance" => {
                // `${instance}`, `${instance:parent}` or `${instance:@PROP.}` (a phandle target).
                let path = match arg {
                    "" => self.path.clone(),
                    "parent" => Tree::parent_path(&self.path).unwrap(),
                    spec => self.target(spec)?.0,
                };
                let inst = self.node(&path).prop("kumiki,instance").and_then(|p| p.str()).ok_or_else(|| self.err("no kumiki,instance"))?;
                format!("p.{inst}")
            }
            "spihost" => {
                let parent = Tree::parent_path(&self.path).unwrap();
                let inst = self.node(&parent).prop("kumiki,instance").and_then(|p| p.str()).unwrap_or("SPI2");
                format!("SpiHost::{}{}", &inst[..1], inst[1..].to_lowercase())
            }
            "pin" => self.pin(arg)?,
            "pinref" if self.backend == Backend::Esp32s3 => format!("{}.reborrow()", self.pin(arg)?),
            "pinref" => return Err(self.err("${pinref} is esp-hal only")),
            "bus" => self.bus_of(&self.path)?,
            "busof" => {
                let p = me.prop(arg).ok_or_else(|| self.err(format!("missing {arg}")))?;
                let target = phandle_target(tree, p).ok_or_else(|| self.err(format!("{arg}: unresolved")))?;
                // The provider device is the target or its nearest ancestor on the bus.
                self.bus_of(&target)?
            }
            "var" => {
                // The bring-up variable of the node a phandle property points to
                // (e.g. the I2S controller a codec sits on).
                let p = me.prop(arg).ok_or_else(|| self.err(format!("missing {arg}")))?;
                let target = phandle_target(tree, p).ok_or_else(|| self.err(format!("{arg}: unresolved")))?;
                label(tree, &target)
            }
            "model" => binding.kumiki.model.clone().ok_or_else(|| self.err("binding has no model"))?,
            "variant" => binding.kumiki.variant.clone().ok_or_else(|| self.err("binding has no variant"))?,
            "dcs_config" => {
                let l = &my_label;
                format!("bind::dcs_config({l}::WIDTH, {l}::HEIGHT, {l}::OFFSET, {l}::INVERT, {l}::RGB_ORDER)")
            }
            "reset" => match me.prop("reset-gpios") {
                None => String::new(),
                Some(p) => match gpios(tree, p)?.first() {
                    Some(Gpio::Chip { .. }) => format!(
                        "{{ let mut rst = {}; bind::reset_pulse(&mut rst, &mut delay); core::mem::forget(rst); }}",
                        self.backend.output_high(&self.pin("reset-gpios")?)
                    ),
                    Some(Gpio::Expander { controller, pin, .. }) => {
                        let cpath = tree.resolve(controller).ok_or_else(|| self.err("unresolved reset controller"))?;
                        let cctx = Ctx { board: self.board, bindings: self.bindings, path: cpath.clone(), backend: self.backend };
                        let cb = self.bindings.for_node(self.node(&cpath)).ok_or_else(|| self.err("controller has no binding"))?;
                        let tpl = cb.kumiki.gpio_pin.as_deref().ok_or_else(|| self.err("controller binding has no gpio-pin"))?;
                        let expr = cctx.expand(&tpl.replace("${line}", &pin.to_string()))?;
                        format!("bind::reset_pulse(&mut {expr}, &mut delay);")
                    }
                    None => String::new(),
                },
            },
            "lines" => {
                let mut lines: std::collections::BTreeMap<u32, &str> = std::collections::BTreeMap::new();
                // Lines other devices reference: outputs at their inactive level.
                tree.root.walk("/", &mut |_, n| {
                    if !is_present(n) {
                        return;
                    }
                    for p in n.props.iter().filter(|p| p.name.ends_with("-gpios") || p.name == "gpios") {
                        for g in gpios(tree, p).unwrap_or_default() {
                            if let Gpio::Expander { controller, pin, flags } = g
                                && controller == my_label
                            {
                                lines.insert(pin, if flags & 1 != 0 { "OutputHigh" } else { "OutputLow" });
                            }
                        }
                    }
                });
                // gpio-hog children win.
                for c in me.children.iter().filter(|c| c.has("gpio-hog")) {
                    let line = c.prop("gpios").and_then(|p| p.u32()).ok_or_else(|| self.err(format!("{}: hog without gpios", c.name)))?;
                    let mode = if c.has("output-high") {
                        "OutputHigh"
                    } else if c.has("output-low") {
                        "OutputLow"
                    } else {
                        "Input"
                    };
                    lines.insert(line, mode);
                }
                let items: Vec<String> = lines.iter().map(|(l, m)| format!("({l}, chips::LineInit::{m})")).collect();
                format!("&[{}]", items.join(", "))
            }
            "regulators" => {
                let mut items = Vec::new();
                if let Some(regs) = me.child("regulators") {
                    for r in &regs.children {
                        let mv = r.prop("regulator-min-microvolt").and_then(|p| p.u32()).map(|uv| uv / 1000);
                        if mv.is_none() && !r.has("regulator-boot-on") {
                            continue;
                        }
                        let name = r.base_name();
                        let variant = format!("{}{}", &name[..1], name[1..].to_lowercase());
                        items.push(format!("(chips::AxpLdo::{variant}, {})", mv.unwrap_or(0)));
                    }
                }
                format!("&[{}]", items.join(", "))
            }
            "touchmap" => {
                let disp = chosen_refs(tree, "kumiki,display").into_iter().next().ok_or_else(|| self.err("no chosen kumiki,display"))?;
                let d = label(tree, &disp);
                let l = &my_label;
                format!(
                    "chips::TouchMap {{ min: ({l}::TOUCHSCREEN_MIN_X.unwrap_or(0), {l}::TOUCHSCREEN_MIN_Y.unwrap_or(0)), size: ({l}::TOUCHSCREEN_SIZE_X.unwrap_or({d}::WIDTH) as i32, {l}::TOUCHSCREEN_SIZE_Y.unwrap_or({d}::HEIGHT) as i32), pixels: ({d}::WIDTH as i32, {d}::HEIGHT as i32) }}"
                )
            }
            "keypin" | "optkeypin" => {
                let child = me.children.iter().find(|c| c.prop("kumiki,button").and_then(|p| p.str()) == Some(arg));
                match (kind, child) {
                    (_, Some(c)) => {
                        let field = crate::pin_field(tree, &self.path, &format!("{}-gpios", c.name), 0, 1);
                        let input = self.backend.input_pullup(&format!("pins.{field}"));
                        if kind == "optkeypin" { format!("Some({input})") } else { input }
                    }
                    ("optkeypin", None) => "None".into(),
                    _ => return Err(self.err(format!("no button {arg:?}"))),
                }
            }
            _ => return Err(self.err(format!("unknown placeholder ${{{ph}}}"))),
        })
    }
}

/// Path of the node a phandle property points to.
fn phandle_target(tree: &Tree, p: &kumiki_dt::Property) -> Option<String> {
    p.value.iter().find_map(|v| match v {
        Value::PathRef(r) => tree.resolve(r),
        Value::Cells(c) => c.iter().find_map(|c| match c {
            Cell::Ref(r) => tree.resolve(r),
            _ => None,
        }),
        _ => None,
    })
}

/// All paths a `/chosen` property references.
fn chosen_refs(tree: &Tree, name: &str) -> Vec<String> {
    let Some(p) = tree.node("/chosen").and_then(|c| c.prop(name)) else { return Vec::new() };
    p.value
        .iter()
        .filter_map(|v| match v {
            Value::PathRef(r) => tree.resolve(r),
            Value::Cells(c) => c.iter().find_map(|c| match c {
                Cell::Ref(r) => tree.resolve(r),
                _ => None,
            }),
            _ => None,
        })
        .collect()
}

/// Emit `pub fn init(…)` for one board (inside its module).
fn emit_init(out: &mut String, board: &Board, bindings: &Bindings, backend: Backend) -> Result<(), Error> {
    let tree = &board.tree;
    let order = init_order(board)?;
    let path_of = |l: &str| -> String {
        let mut found = String::new();
        tree.root.walk("/", &mut |p, n| {
            if found.is_empty() && (n.labels.iter().any(|x| x == l) || (n.labels.is_empty() && ident_lower(n.base_name()) == l)) {
                found = p.to_string();
            }
        });
        found
    };
    writeln!(out, "\n    /// Bring the board up in `INIT_ORDER` (generated from the binding templates).").unwrap();
    writeln!(out, "    #[inline(never)]").unwrap();
    writeln!(out, "    #[allow(unused_variables, unused_mut, unused_braces, clippy::let_unit_value)]").unwrap();
    writeln!(out, "    pub fn init(p: Peripherals{}) -> Result<Devices, InitError> {{", backend.init_args().0).unwrap();
    writeln!(out, "        let mut pins = take_pins_{}!(p);", board.module).unwrap();
    writeln!(out, "        let spare = take_spare!(p);").unwrap();
    writeln!(out, "        let mut delay = {};", backend.delay_new()).unwrap();
    let mut groups_done: Vec<String> = Vec::new();
    for l in &order {
        let path = path_of(l);
        let n = tree.node(&path).unwrap();
        let b = bindings.for_node(n).unwrap();
        if let Some(group) = n.prop("kumiki,probe-group").and_then(|p| p.str()) {
            if groups_done.iter().any(|g| g == group) {
                continue;
            }
            groups_done.push(group.to_string());
            // All candidates of the group, in tree order.
            let mut members = Vec::new();
            tree.root.walk("/", &mut |p, m| {
                if is_present(m) && m.prop("kumiki,probe-group").and_then(|q| q.str()) == Some(group) {
                    members.push(p.to_string());
                }
            });
            writeln!(out, "        // probe group `{group}`: first candidate that answers (retried for slow starters)").unwrap();
            writeln!(out, "        let {group}_group = {{\n            let mut found = None;\n            for _ in 0..8 {{").unwrap();
            for m in &members {
                let ctx = Ctx { board, bindings, path: m.clone(), backend };
                let mb = bindings.for_node(tree.node(m).unwrap()).unwrap();
                let probe = mb.kumiki.probe.as_deref().ok_or_else(|| ctx.err("candidate binding has no probe"))?;
                let init = init_template(mb, backend).ok_or_else(|| ctx.err("binding has no init"))?;
                writeln!(out, "                if {} {{", ctx.expand(probe)?).unwrap();
                writeln!(out, "                    found = Some({});\n                    break;\n                }}", ctx.expand(init.trim())?).unwrap();
            }
            writeln!(out, "                delay.delay_ms(10);\n            }}\n            found\n        }};").unwrap();
            continue;
        }
        let ctx = Ctx { board, bindings, path: path.clone(), backend };
        let tpl = init_template(b, backend).ok_or_else(|| ctx.err(format!("binding {} has no init template", b.compatible)))?;
        let code = ctx.expand(tpl.trim())?;
        let code = code.replace('\n', "\n        ");
        writeln!(out, "        // {path} ({})", b.compatible).unwrap();
        writeln!(out, "        let mut {l} = {code};").unwrap();
    }
    // Devices
    let var_of = |path: &str| -> String {
        let n = tree.node(path).unwrap();
        match n.prop("kumiki,probe-group").and_then(|p| p.str()) {
            Some(g) => format!("{g}_group"),
            None => label(tree, path),
        }
    };
    let class_node = |class: &str| -> Option<String> {
        let mut found = None;
        tree.root.walk("/", &mut |p, n| {
            if found.is_none() && is_present(n) && p != "/" && bindings.for_node(n).and_then(|b| b.kumiki.class.as_deref()) == Some(class) {
                found = Some(p.to_string());
            }
        });
        found
    };
    let display = chosen_refs(tree, "kumiki,display").into_iter().next().ok_or_else(|| Error(format!("{}: no chosen kumiki,display", board.source.display())))?;
    let touch = match chosen_refs(tree, "kumiki,touch").first() {
        Some(t) if tree.node(t).unwrap().has("kumiki,probe-group") => var_of(t),
        Some(t) => format!("Some({})", var_of(t)),
        None => "None".into(),
    };
    let imu = chosen_refs(tree, "kumiki,imu").first().map(|p| var_of(p)).unwrap_or_else(|| "None".into());
    let power = chosen_refs(tree, "kumiki,power").first().map(|p| format!("Some({})", var_of(p))).unwrap_or_else(|| "None".into());
    // RTC init templates yield an Option (the chip may not answer).
    let rtc = chosen_refs(tree, "kumiki,rtc").first().map(|p| var_of(p)).unwrap_or_else(|| "None".into());
    let speaker = chosen_refs(tree, "kumiki,speaker").first().map(|p| var_of(p)).unwrap_or_else(|| "None".into());
    let microphone = chosen_refs(tree, "kumiki,microphone").first().map(|p| var_of(p)).unwrap_or_else(|| "None".into());
    let backlight = class_node("backlight").map(|p| var_of(&p)).unwrap_or_else(|| "AnyBacklight::None".into());
    let buttons = class_node("buttons").map(|p| var_of(&p)).ok_or_else(|| Error(format!("{}: no buttons node", board.source.display())))?;
    writeln!(out, "        Ok(Devices {{").unwrap();
    writeln!(out, "            board: super::BoardId::{},", variant_of(board.model())).unwrap();
    writeln!(out, "            display: {},", var_of(&display)).unwrap();
    writeln!(out, "            backlight: {backlight},\n            touch: {touch},\n            imu: {imu},\n            power: {power},\n            rtc: {rtc},\n            speaker: {speaker},\n            microphone: {microphone},").unwrap();
    writeln!(out, "            buttons: {buttons},\n            spare,\n        }})\n    }}").unwrap();
    Ok(())
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
fn emit_detect(out: &mut String, board: &Board, backend: Backend) -> Result<(), Error> {
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
            match backend {
                Backend::Esp32s3 => {
                    writeln!(out, "            super::probe::i2c_bus_recover(p.GPIO{scl}.reborrow(), p.GPIO{sda}.reborrow());").unwrap();
                    writeln!(
                        out,
                        "            let v = super::probe::i2c_read_reg(p.{inst}.reborrow(), p.GPIO{sda}.reborrow(), p.GPIO{scl}.reborrow(), {addr:#04x}, {reg:#04x});"
                    )
                    .unwrap();
                }
                Backend::P4v1 => {
                    // Peripherals are only borrowed: the probe works on stolen handles.
                    writeln!(out, "            let v = super::probe::i2c_read_reg(p, {inst:?}, {sda}, {scl}, {addr:#04x}, {reg:#04x});").unwrap();
                }
            }
            writeln!(out, "            log.push({model:?}, {pname:?}, v.map(|b| [b, 0, 0]), 1);").unwrap();
            if comp.contains(&"kumiki,probe-i2c-reg") {
                let expect = probe.prop("expect").and_then(|p| p.u32()).unwrap_or(0);
                writeln!(out, "            ok = v == Some({expect:#04x});").unwrap();
            } else {
                writeln!(out, "            ok = v.is_some();").unwrap();
            }
            writeln!(out, "        }}").unwrap();
        } else if comp.contains(&"kumiki,probe-dcs-id-3wire") && backend == Backend::Esp32s3 {
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

/// Device classes that get an `Any<Class>` enum, with the enum name.
const ANY_CLASSES: &[(&str, &str, &str)] = &[
    ("display", "AnyDisplay", "Display"),
    ("touch", "AnyTouch", "Touch panel"),
    ("imu", "AnyImu", "IMU"),
    ("power", "AnyPower", "Power / battery information"),
    ("backlight", "AnyBacklight", "Backlight"),
    ("rtc", "AnyRtc", "Real-time clock"),
    ("speaker", "AnySpeaker", "Audio output"),
    ("microphone", "AnyMicrophone", "Audio input"),
];

/// Emit the `Any<Class>` enums: one variant per binding `kumiki.variant`
/// found on the family's boards, holding `kumiki.type`. Each enum gets
/// `name()` and a dispatch macro `any_<class>!(value, v => expr)` (plus
/// `, none => expr` when there is a unit variant) for the hand-written trait
/// impls, so adding a device needs no edit there.
fn emit_any(out: &mut String, boards: &[Board], bindings: &Bindings, opt: &Options) -> Result<(), Error> {
    struct Var {
        name: String,
        ty: Option<String>,
        compatible: String,
        boards: Vec<String>,
    }
    writeln!(out, "use super::*;\n").unwrap();
    for (class, enum_name, doc) in ANY_CLASSES {
        let mut vars: Vec<Var> = Vec::new();
        for b in boards {
            b.tree.root.walk("/", &mut |path, n| {
                if path == "/" || !is_present(n) {
                    return;
                }
                let Some(bd) = bindings.for_node(n) else { return };
                if bd.kumiki.class.as_deref() != Some(class) {
                    return;
                }
                let Some(v) = bd.kumiki.variant.clone() else { return };
                let ty = bd.kumiki.ty_for.get(opt.backend.key()).or(bd.kumiki.ty.as_ref()).cloned();
                match vars.iter_mut().find(|x| x.name == v) {
                    Some(x) => {
                        if !x.boards.contains(&b.model().to_string()) {
                            x.boards.push(b.model().to_string());
                        }
                    }
                    None => vars.push(Var { name: v, ty, compatible: bd.compatible.clone(), boards: vec![b.model().to_string()] }),
                }
            });
        }
        // The generated `init` falls back to `AnyBacklight::None`.
        if *class == "backlight" && !vars.iter().any(|v| v.name == "None") {
            vars.push(Var { name: "None".into(), ty: None, compatible: String::new(), boards: Vec::new() });
        }
        if let Some(v) = vars.iter().find(|v| v.ty.is_none() && v.name != "None") {
            return Err(Error(format!("binding {}: variant {} has no kumiki.type", v.compatible, v.name)));
        }
        writeln!(out, "/// {doc} of any board in this family (generated from the bindings).").unwrap();
        writeln!(out, "pub enum {enum_name} {{").unwrap();
        for v in &vars {
            match &v.ty {
                Some(t) => {
                    writeln!(out, "    /// `{}` ({}).\n    {}({t}),", v.compatible, v.boards.join(", "), v.name).unwrap();
                }
                None => writeln!(out, "    /// Not controllable.\n    {},", v.name).unwrap(),
            }
        }
        writeln!(out, "}}\n").unwrap();
        // name(): the part number from the compatible.
        writeln!(out, "impl {enum_name} {{\n    /// Device part number (from the compatible).\n    pub fn name(&self) -> &'static str {{").unwrap();
        if vars.is_empty() {
            writeln!(out, "        match *self {{}}").unwrap();
        } else {
            writeln!(out, "        match self {{").unwrap();
            for v in &vars {
                let part = v.compatible.split(',').nth(1).unwrap_or("none").to_uppercase();
                let pat = if v.ty.is_some() { format!("{enum_name}::{}(_)", v.name) } else { format!("{enum_name}::{}", v.name) };
                writeln!(out, "            {pat} => {part:?},").unwrap();
            }
            writeln!(out, "        }}").unwrap();
        }
        writeln!(out, "    }}\n}}\n").unwrap();
        // Dispatch macro.
        let mac = format!("any_{class}");
        let has_unit = vars.iter().any(|v| v.ty.is_none());
        writeln!(out, "/// `{mac}!(value, v => expr{})`: `expr` for every variant of [`{enum_name}`].", if has_unit { ", none => expr" } else { "" }).unwrap();
        writeln!(out, "#[allow(unused_macros)]\nmacro_rules! {mac} {{").unwrap();
        let path = format!("{}::{enum_name}", opt.module_path);
        if has_unit {
            writeln!(out, "    ($s:expr, $v:ident => $e:expr, none => $n:expr) => {{\n        match $s {{").unwrap();
        } else {
            writeln!(out, "    ($s:expr, $v:ident => $e:expr) => {{\n        match $s {{").unwrap();
        }
        if vars.is_empty() {
            // `$s` is a reference; an empty enum needs the place itself.
            out.truncate(out.len() - "        match $s {\n".len());
            writeln!(out, "        match *$s {{").unwrap();
        }
        for v in &vars {
            match v.ty {
                Some(_) => writeln!(out, "            {path}::{}($v) => $e,", v.name).unwrap(),
                None => writeln!(out, "            {path}::{} => $n,", v.name).unwrap(),
            }
        }
        writeln!(out, "        }}\n    }};\n}}\n").unwrap();
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

"#,
    );
    writeln!(out, "use {};", opt.backend.peripherals()).unwrap();
    out.push_str(
        r#"

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
    out.push_str(opt.backend.prelude());
    out.push('\n');
    emit_any(&mut out, boards, bindings, opt)?;
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
        emit_detect(&mut out, b, opt.backend)?;
        writeln!(out, "        if ok {{\n            return Ok(BoardId::{});\n        }}\n    }}", variant_of(b.model())).unwrap();
    }
    writeln!(out, "    Err(log)\n}}\n").unwrap();

    // Pin bundles and macros.
    for b in boards {
        let pins = chip_pins(b);
        writeln!(out, "/// Move the chip GPIOs used by {} out of `Peripherals`.", b.model()).unwrap();
        writeln!(out, "macro_rules! take_pins_{} {{\n    ($p:ident) => {{\n        {}::{}::Pins {{", b.module, opt.module_path, b.module).unwrap();
        for (field, pin) in &pins {
            writeln!(out, "            {field}: {},", opt.backend.pin_take(*pin)).unwrap();
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
        writeln!(out, "    #[allow(unused_imports)]\n    use super::prelude::*;\n").unwrap();
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
            writeln!(out, "        pub {field}: {},", opt.backend.pin_type(*pin)).unwrap();
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
        emit_init(&mut out, b, bindings, opt.backend)?;
        writeln!(out, "}}\n").unwrap();
    }
    // Dispatch.
    writeln!(out, "/// Bring up the detected board.").unwrap();
    writeln!(
        out,
        "pub fn init_board(id: BoardId, p: Peripherals{}) -> Result<prelude::Devices, prelude::InitError> {{\n    match id {{",
        opt.backend.init_args_dispatch()
    )
    .unwrap();
    for b in boards {
        writeln!(out, "        BoardId::{} => {}::init(p{}),", variant_of(b.model()), b.module, opt.backend.init_args().1).unwrap();
    }
    writeln!(out, "    }}\n}}").unwrap();
    // Trim the final blank line for a stable file ending.
    while out.ends_with("\n\n") {
        out.pop();
    }
    Ok(out)
}
