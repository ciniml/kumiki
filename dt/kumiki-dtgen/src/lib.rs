//! # kumiki-dtgen
//!
//! Turns board descriptions written in Device Tree Source into typed Rust:
//!
//! 1. load the DTS ([`kumiki_dt`]) and the YAML bindings ([`Bindings`]);
//! 2. validate: every enabled node with a `compatible` must have a binding,
//!    required properties must exist with the declared types, references
//!    must resolve, chip GPIOs must exist and must not be used twice;
//! 3. compute the device initialization order from the reference graph
//!    (bus parent, phandles, `*-gpios` on expanders, `kumiki,depends-on`);
//! 4. emit Rust for a chip family ([`gen_esp`]: ESP32-S3 on esp-hal, ESP32-P4
//!    rev v1 on kumiki-p4v1-hal).
//!
//! v0 generates board data (constants, pin bundles, detection plan, init
//! order). Device bring-up code still lives in the board crate; generating it
//! from binding templates is the next step (doc/11 M3).

pub mod gen_esp;
pub mod soc_dtsi;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use kumiki_dt::{Cell, Node, Tree, Value};
use serde::Deserialize;

/// Generator error.
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

impl From<kumiki_dt::Error> for Error {
    fn from(e: kumiki_dt::Error) -> Self {
        Error(e.to_string())
    }
}

/// Property type declared in a binding.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PropType {
    /// One u32 cell.
    Int,
    /// One cell interpreted as i32.
    Sint,
    /// Present / absent.
    Boolean,
    /// One string.
    String,
    /// String list.
    StringArray,
    /// u32 list.
    Array,
    /// `[ bytes ]`.
    Uint8Array,
    /// One phandle.
    Phandle,
    /// Phandles with arguments.
    PhandleArray,
    /// GPIO specifiers `<&controller pin flags>…`.
    Gpios,
}

/// A property declaration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropSpec {
    /// Type.
    #[serde(rename = "type")]
    pub ty: PropType,
    /// Required.
    #[serde(default)]
    pub required: bool,
    /// Documentation.
    #[serde(default)]
    pub description: Option<String>,
}

/// kumiki-specific binding metadata.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KumikiMeta {
    /// Parent controller kind the device sits on (`i2c`, `spi`).
    #[serde(default)]
    pub bus: Option<String>,
    /// Controller kind this node provides.
    #[serde(default)]
    pub controller: Option<String>,
    /// kumiki-class trait.
    #[serde(default)]
    pub class: Option<String>,
    /// Properties of child nodes (e.g. gpio-keys entries): name → type.
    #[serde(default, rename = "child-props")]
    pub child_props: BTreeMap<String, PropType>,
    /// Bring-up expression template (backend-specific placeholders).
    #[serde(default)]
    pub init: Option<String>,
    /// Backend-specific bring-up templates (`p4v1: ...`), used instead of
    /// `init` by that backend. Bindings whose `init` uses only portable
    /// items (embedded-hal, kumiki drivers) need none.
    #[serde(default, rename = "init-for")]
    pub init_for: BTreeMap<String, String>,
    /// Presence test for `status = "fail-needs-probe"` candidates.
    #[serde(default)]
    pub probe: Option<String>,
    /// `OutputPin` expression for one line of this gpio-controller (`${line}`).
    #[serde(default, rename = "gpio-pin")]
    pub gpio_pin: Option<String>,
    /// Driver model type substituted for `${model}`.
    #[serde(default)]
    pub model: Option<String>,
    /// `Any*` enum variant substituted for `${variant}`; a device with a
    /// variant becomes one in the family's generated `Any<Class>` enum.
    #[serde(default)]
    pub variant: Option<String>,
    /// Rust type the variant holds (relative to the family module; none =
    /// unit variant).
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    /// Backend-specific `type` (like `init-for`).
    #[serde(default, rename = "type-for")]
    pub ty_for: BTreeMap<String, String>,
}

/// One binding.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Compatible string.
    pub compatible: String,
    /// Documentation.
    #[serde(default)]
    pub description: Option<String>,
    /// kumiki metadata.
    #[serde(default)]
    pub kumiki: KumikiMeta,
    /// Declared properties.
    #[serde(default)]
    pub properties: BTreeMap<String, PropSpec>,
}

/// All bindings, by compatible.
#[derive(Debug, Default)]
pub struct Bindings(pub HashMap<String, Binding>);

/// Properties every node may carry without declaring them.
const COMMON_PROPS: &[&str] = &[
    "compatible",
    "status",
    "#address-cells",
    "#size-cells",
    "phandle",
    "kumiki,depends-on",
    "kumiki,optional",
    "kumiki,probe-group",
];

impl Bindings {
    /// Load every `*.yaml` file in `dir` (each a list of bindings).
    pub fn load_dir(dir: &Path) -> Result<Self, Error> {
        let mut out = HashMap::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| Error(format!("{}: {e}", dir.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(|e| Error(format!("{}: {e}", f.display())))?;
            let list: Vec<Binding> =
                serde_yaml::from_str(&text).map_err(|e| Error(format!("{}: {e}", f.display())))?;
            for b in list {
                if out.insert(b.compatible.clone(), b).is_some() {
                    return Err(Error(format!("{}: duplicate binding", f.display())));
                }
            }
        }
        Ok(Self(out))
    }

    /// Binding for a node: the first of its `compatible` strings that has one.
    pub fn for_node(&self, n: &Node) -> Option<&Binding> {
        n.compatible().iter().find_map(|c| self.0.get(*c))
    }
}

/// A loaded board.
#[derive(Debug)]
pub struct Board {
    /// Rust module name (file stem, e.g. `cores3`).
    pub module: String,
    /// Source path (for messages / headers).
    pub source: PathBuf,
    /// The merged tree.
    pub tree: Tree,
    /// Files read.
    pub files: Vec<PathBuf>,
}

impl Board {
    /// Load a board DTS with the given include directories.
    pub fn load(path: &Path, include_dirs: &[PathBuf]) -> Result<Self, Error> {
        let (tree, files) = kumiki_dt::load(path, include_dirs)?;
        let module = path.file_stem().unwrap().to_string_lossy().replace('-', "_");
        Ok(Self { module, source: path.to_path_buf(), tree, files })
    }

    /// `model`.
    pub fn model(&self) -> &str {
        self.tree.root.prop("model").and_then(|p| p.str()).unwrap_or(&self.module)
    }

    /// First root `compatible`.
    pub fn compatible(&self) -> &str {
        self.tree.root.prop("compatible").and_then(|p| p.str()).unwrap_or("")
    }
}

/// A GPIO reference resolved from a `*-gpios` cell group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gpio {
    /// A chip GPIO.
    Chip { pin: u32, flags: u32 },
    /// A pin of another gpio-controller node (IO expander), by node label.
    Expander { controller: String, pin: u32, flags: u32 },
}

/// Is this node enabled (okay, or a second-source candidate)?
pub fn is_present(n: &Node) -> bool {
    n.is_okay() || n.status() == "fail-needs-probe"
}

fn label_of(tree: &Tree, path: &str) -> String {
    let n = tree.node(path).unwrap();
    n.labels.first().cloned().unwrap_or_else(|| n.base_name().replace(['-', ','], "_"))
}

/// Resolve the GPIO specifiers of a property (`#gpio-cells = <2>` assumed for every controller).
pub fn gpios(tree: &Tree, prop: &kumiki_dt::Property) -> Result<Vec<Gpio>, Error> {
    let cells = prop.cells();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let Cell::Ref(r) = &cells[i] else {
            return Err(Error(format!("{}: expected a phandle at cell {i}", prop.name)));
        };
        let path = tree.resolve(r).ok_or_else(|| Error(format!("{}: unresolved &{r}", prop.name)))?;
        let ctrl = tree.node(&path).unwrap();
        let ncells = ctrl.prop("#gpio-cells").and_then(|p| p.u32()).unwrap_or(2) as usize;
        let args: Vec<u32> = cells[i + 1..(i + 1 + ncells).min(cells.len())]
            .iter()
            .map(|c| match c {
                Cell::Num(n) => Ok(*n),
                Cell::Ref(_) => Err(Error(format!("{}: phandle where a number was expected", prop.name))),
            })
            .collect::<Result<_, _>>()?;
        if args.len() != ncells {
            return Err(Error(format!("{}: expected {ncells} cells after &{r}", prop.name)));
        }
        let (pin, flags) = (args[0], args.get(1).copied().unwrap_or(0));
        if ctrl.compatible().iter().any(|c| c.ends_with("-gpio")) {
            out.push(Gpio::Chip { pin, flags });
        } else {
            out.push(Gpio::Expander { controller: label_of(tree, &path), pin, flags });
        }
        i += 1 + ncells;
    }
    Ok(out)
}

/// Properties whose reference means "the target comes *after* me" (the
/// backlight is switched on once the panel has been initialized and cleared).
const REVERSE_DEP_PROPS: &[&str] = &["backlight"];

/// References (as paths) a node makes through its properties, excluding
/// [`REVERSE_DEP_PROPS`].
fn refs_of(tree: &Tree, n: &Node) -> Vec<String> {
    let mut out = Vec::new();
    for p in n.props.iter().filter(|p| !REVERSE_DEP_PROPS.contains(&p.name.as_str())) {
        for v in &p.value {
            match v {
                Value::Cells(cs) => out.extend(cs.iter().filter_map(|c| match c {
                    Cell::Ref(r) => tree.resolve(r),
                    _ => None,
                })),
                Value::PathRef(r) => out.extend(tree.resolve(r)),
                _ => {}
            }
        }
    }
    out
}

/// Validate a board against the bindings. Returns all problems found.
pub fn validate(board: &Board, bindings: &Bindings) -> Vec<String> {
    let mut errs = Vec::new();
    let tree = &board.tree;
    let ngpios = chip_gpio_count(tree);
    let mut used_pins: BTreeMap<u32, String> = BTreeMap::new();
    tree.root.walk("/", &mut |path, n| {
        if !is_present(n) {
            return;
        }
        // References must resolve (all value kinds).
        for p in &n.props {
            for v in &p.value {
                let mut check = |r: &str| {
                    if tree.resolve(r).is_none() {
                        errs.push(format!("{path}: {}: unresolved reference &{r}", p.name));
                    }
                };
                match v {
                    Value::Cells(cs) => cs.iter().for_each(|c| {
                        if let Cell::Ref(r) = c {
                            check(r)
                        }
                    }),
                    Value::PathRef(r) => check(r),
                    _ => {}
                }
            }
        }
        if n.compatible().is_empty() || path == "/" {
            return; // structural node (soc, regulators, gpio-keys entries, chosen) or the board root
        }
        let Some(b) = bindings.for_node(n) else {
            errs.push(format!("{path}: no binding for {:?}", n.compatible()));
            return;
        };
        for (name, spec) in &b.properties {
            let p = n.prop(name);
            if spec.required && p.is_none() {
                errs.push(format!("{path}: missing required property {name}"));
            }
            if let Some(p) = p {
                let ok = match spec.ty {
                    PropType::Boolean => p.value.is_empty(),
                    PropType::Int | PropType::Sint => p.cells().len() == 1 && matches!(p.cells()[0], Cell::Num(_)),
                    PropType::Array => !p.u32s().is_empty() && p.u32s().len() == p.cells().len(),
                    PropType::String => p.strs().len() == 1 && p.value.len() == 1,
                    PropType::StringArray => !p.strs().is_empty() && p.strs().len() == p.value.len(),
                    PropType::Uint8Array => !p.bytes().is_empty(),
                    PropType::Phandle => {
                        matches!(p.value.as_slice(), [Value::PathRef(_)])
                            || matches!(p.value.as_slice(), [Value::Cells(c)] if matches!(c.as_slice(), [Cell::Ref(_)]))
                    }
                    PropType::PhandleArray | PropType::Gpios => !p.cells().is_empty(),
                };
                if !ok {
                    errs.push(format!("{path}: {name} is not a valid {:?}", spec.ty));
                }
            }
        }
        for p in &n.props {
            if !b.properties.contains_key(&p.name) && !COMMON_PROPS.contains(&p.name.as_str()) {
                errs.push(format!("{path}: property {} not declared in binding {}", p.name, b.compatible));
            }
        }
        // GPIOs: chip pins must exist and be used once.
        let mut gpio_props: Vec<(String, &kumiki_dt::Property)> =
            n.props.iter().filter(|p| p.name == "gpios" || p.name.ends_with("-gpios")).map(|p| (p.name.clone(), p)).collect();
        // gpio-keys style child entries (gpio-hog children use controller-local specifiers)
        for c in n.children.iter().filter(|c| !c.has("gpio-hog")) {
            for p in c.props.iter().filter(|p| p.name == "gpios") {
                gpio_props.push((format!("{}/gpios", c.name), p));
            }
        }
        for (pname, p) in gpio_props {
            match gpios(tree, p) {
                Ok(list) => {
                    for g in list {
                        if let Gpio::Chip { pin, .. } = g {
                            if pin >= ngpios {
                                errs.push(format!("{path}: {pname}: GPIO{pin} does not exist (ngpios = {ngpios})"));
                            }
                            let user = format!("{path}:{pname}");
                            if let Some(prev) = used_pins.insert(pin, user.clone()) {
                                errs.push(format!("GPIO{pin} used twice: {prev} and {user}"));
                            }
                        }
                    }
                }
                Err(e) => errs.push(format!("{path}: {e}")),
            }
        }
    });
    errs
}

/// `ngpios` of the chip GPIO controller.
pub fn chip_gpio_count(tree: &Tree) -> u32 {
    let mut n = 0;
    tree.root.walk("/", &mut |_, node| {
        if node.compatible().iter().any(|c| c.ends_with("-gpio")) {
            n = node.prop("ngpios").and_then(|p| p.u32()).unwrap_or(0);
        }
    });
    n
}

/// Name of the `Pins` field for entry `index` of GPIO property `prop` of the
/// node at `path` (child entries such as gpio-keys use `"<child>-gpios"`).
/// Single-entry properties get no index suffix.
pub fn pin_field(tree: &Tree, path: &str, prop: &str, index: usize, entries: usize) -> String {
    let label = label_of(tree, path);
    let base = prop.trim_end_matches("-gpios").trim_end_matches("gpios").replace('-', "_");
    match (base.is_empty(), entries) {
        (true, _) => label,
        (false, 1) => format!("{label}_{base}"),
        (false, _) => format!("{label}_{base}{index}"),
    }
}

/// Every chip GPIO the board uses, with its `Pins` field name.
pub fn chip_pins(board: &Board) -> Vec<(String, u32)> {
    let tree = &board.tree;
    let mut out = Vec::new();
    tree.root.walk("/", &mut |path, n| {
        if !is_present(n) || n.compatible().is_empty() || path == "/" {
            return;
        }
        let mut props: Vec<(String, &kumiki_dt::Property)> = n
            .props
            .iter()
            .filter(|p| p.name == "gpios" || p.name.ends_with("-gpios"))
            .map(|p| (p.name.clone(), p))
            .collect();
        for c in n.children.iter().filter(|c| !c.has("gpio-hog")) {
            for p in c.props.iter().filter(|p| p.name == "gpios") {
                props.push((format!("{}-gpios", c.name), p));
            }
        }
        for (pname, p) in props {
            let list = gpios(tree, p).unwrap_or_default();
            let n_entries = list.len();
            for (i, g) in list.into_iter().enumerate() {
                if let Gpio::Chip { pin, .. } = g {
                    out.push((pin_field(tree, path, &pname, i, n_entries), pin));
                }
            }
        }
    });
    out
}

/// Device initialization order: Kahn's algorithm over the dependency graph,
/// stable with respect to tree order. Returns node labels.
pub fn init_order(board: &Board) -> Result<Vec<String>, Error> {
    let tree = &board.tree;
    let mut nodes: Vec<String> = Vec::new();
    tree.root.walk("/", &mut |path, n| {
        if is_present(n) && !n.compatible().is_empty() && path != "/" {
            let gpio_ctrl_chip = n.compatible().iter().any(|c| c.ends_with("-gpio") || *c == "simple-bus");
            if !gpio_ctrl_chip && !n.compatible().iter().any(|c| c.starts_with("kumiki,probe") || *c == "kumiki,board-detect") {
                nodes.push(path.to_string());
            }
        }
    });
    let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, p)| (p.as_str(), i)).collect();
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, path) in nodes.iter().enumerate() {
        let n = tree.node(path).unwrap();
        // Nearest ancestor that is itself a device (the bus).
        let mut p = Tree::parent_path(path);
        while let Some(pp) = p {
            if let Some(&j) = index.get(pp.as_str()) {
                deps[i].push(j);
                break;
            }
            p = Tree::parent_path(&pp);
        }
        // Reverse edges: the referenced node depends on this one.
        for p in n.props.iter().filter(|p| REVERSE_DEP_PROPS.contains(&p.name.as_str())) {
            for c in p.cells() {
                if let Cell::Ref(r) = c
                    && let Some(&j) = tree.resolve(&r).as_deref().and_then(|t| index.get(t))
                {
                    deps[j].push(i);
                }
            }
        }
        for r in refs_of(tree, n) {
            // A reference into a sub-node (regulator) depends on its device.
            let mut q = Some(r);
            while let Some(qq) = q {
                if let Some(&j) = index.get(qq.as_str()) {
                    if j != i {
                        deps[i].push(j);
                    }
                    break;
                }
                q = Tree::parent_path(&qq);
            }
        }
    }
    let mut done = vec![false; nodes.len()];
    let mut order = Vec::new();
    while order.len() < nodes.len() {
        let next = (0..nodes.len()).find(|&i| !done[i] && deps[i].iter().all(|&d| done[d]));
        match next {
            Some(i) => {
                done[i] = true;
                order.push(label_of(tree, &nodes[i]));
            }
            None => {
                let stuck: Vec<String> = (0..nodes.len()).filter(|&i| !done[i]).map(|i| nodes[i].clone()).collect();
                return Err(Error(format!("{}: dependency cycle among {stuck:?}", board.source.display())));
            }
        }
    }
    Ok(order)
}
