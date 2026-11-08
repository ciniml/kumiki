//! Device tree model.

/// One cell of a `< … >` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// A 32-bit number (expressions are already evaluated).
    Num(u32),
    /// A phandle reference `&label` or `&{/path}`.
    Ref(String),
}

/// One comma-separated part of a property value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `"string"`
    Str(String),
    /// `< cells >`
    Cells(Vec<Cell>),
    /// `[ bytes ]`
    Bytes(Vec<u8>),
    /// A path reference outside `< >`: `&label`.
    PathRef(String),
}

/// A property.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    /// Name.
    pub name: String,
    /// Value parts (empty for boolean properties).
    pub value: Vec<Value>,
}

impl Property {
    /// All cells, concatenated.
    pub fn cells(&self) -> Vec<Cell> {
        self.value
            .iter()
            .flat_map(|v| match v {
                Value::Cells(c) => c.clone(),
                _ => Vec::new(),
            })
            .collect()
    }
    /// All numeric cells (phandles skipped).
    pub fn u32s(&self) -> Vec<u32> {
        self.cells()
            .into_iter()
            .filter_map(|c| match c {
                Cell::Num(n) => Some(n),
                Cell::Ref(_) => None,
            })
            .collect()
    }
    /// First numeric cell.
    pub fn u32(&self) -> Option<u32> {
        self.u32s().first().copied()
    }
    /// The strings of a string-list property.
    pub fn strs(&self) -> Vec<&str> {
        self.value
            .iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }
    /// First string.
    pub fn str(&self) -> Option<&str> {
        self.strs().first().copied()
    }
    /// Bytes of a `[ … ]` property.
    pub fn bytes(&self) -> Vec<u8> {
        self.value
            .iter()
            .flat_map(|v| match v {
                Value::Bytes(b) => b.clone(),
                _ => Vec::new(),
            })
            .collect()
    }
}

/// A node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Full name including unit address (`pmic@34`); `/` for the root.
    pub name: String,
    /// Labels.
    pub labels: Vec<String>,
    /// Properties in definition order.
    pub props: Vec<Property>,
    /// Children in definition order.
    pub children: Vec<Node>,
}

impl Node {
    /// Name without unit address.
    pub fn base_name(&self) -> &str {
        self.name.split('@').next().unwrap_or(&self.name)
    }
    /// Unit address part, if any.
    pub fn unit(&self) -> Option<&str> {
        self.name.split_once('@').map(|(_, u)| u)
    }
    /// Property by name.
    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name == name)
    }
    /// Whether a (boolean or valued) property exists.
    pub fn has(&self, name: &str) -> bool {
        self.prop(name).is_some()
    }
    /// Child by full name.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    /// `compatible` strings.
    pub fn compatible(&self) -> Vec<&str> {
        self.prop("compatible").map(|p| p.strs()).unwrap_or_default()
    }
    /// `status` (defaults to "okay").
    pub fn status(&self) -> &str {
        self.prop("status").and_then(|p| p.str()).unwrap_or("okay")
    }
    /// Enabled (`status = "okay"` or absent).
    pub fn is_okay(&self) -> bool {
        matches!(self.status(), "okay" | "ok")
    }

    /// Merge `other` into `self`: properties override by name, children merge
    /// recursively by full name, labels accumulate.
    pub fn merge(&mut self, other: Node) {
        for l in other.labels {
            if !self.labels.contains(&l) {
                self.labels.push(l);
            }
        }
        for p in other.props {
            match self.props.iter_mut().find(|q| q.name == p.name) {
                Some(q) => *q = p,
                None => self.props.push(p),
            }
        }
        for c in other.children {
            match self.children.iter_mut().find(|d| d.name == c.name) {
                Some(d) => d.merge(c),
                None => self.children.push(c),
            }
        }
    }

    /// Visit every node with its path.
    pub fn walk<'a>(&'a self, path: &str, f: &mut impl FnMut(&str, &'a Node)) {
        f(path, self);
        for c in &self.children {
            let p = if path == "/" { format!("/{}", c.name) } else { format!("{path}/{}", c.name) };
            c.walk(&p, f);
        }
    }
}

/// A whole device tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    /// Root node.
    pub root: Node,
}

impl Tree {
    /// Node at an absolute path (`/soc/i2c@60013000`).
    pub fn node(&self, path: &str) -> Option<&Node> {
        let mut n = &self.root;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            n = n.children.iter().find(|c| c.name == part || (c.unit().is_none() && c.name == part))?;
        }
        Some(n)
    }

    /// Path of the node carrying `label`.
    pub fn label_path(&self, label: &str) -> Option<String> {
        let mut found = None;
        self.root.walk("/", &mut |p, n| {
            if found.is_none() && n.labels.iter().any(|l| l == label) {
                found = Some(p.to_string());
            }
        });
        found
    }

    /// Resolve a reference (`label` or `{/path}` / `/path`) to a path.
    pub fn resolve(&self, r: &str) -> Option<String> {
        let r = r.trim_start_matches('&');
        if let Some(p) = r.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            return self.node(p).map(|_| p.to_string());
        }
        if r.starts_with('/') {
            return self.node(r).map(|_| r.to_string());
        }
        self.label_path(r)
    }

    /// Node by reference.
    pub fn by_ref(&self, r: &str) -> Option<&Node> {
        self.resolve(r).and_then(|p| self.node(&p))
    }

    /// Mutable node by path.
    pub fn node_mut(&mut self, path: &str) -> Option<&mut Node> {
        let mut n = &mut self.root;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            n = n.children.iter_mut().find(|c| c.name == part)?;
        }
        Some(n)
    }

    /// Parent path of a path.
    pub fn parent_path(path: &str) -> Option<String> {
        if path == "/" {
            return None;
        }
        let (p, _) = path.rsplit_once('/')?;
        Some(if p.is_empty() { "/".into() } else { p.into() })
    }

    /// `/chosen` property resolved to a path (`kumiki,display = &lcd;`).
    pub fn chosen(&self, name: &str) -> Option<String> {
        let chosen = self.node("/chosen")?;
        let p = chosen.prop(name)?;
        p.value.iter().find_map(|v| match v {
            Value::PathRef(r) => self.resolve(r),
            Value::Cells(c) => c.iter().find_map(|c| match c {
                Cell::Ref(r) => self.resolve(r),
                _ => None,
            }),
            Value::Str(s) => self.resolve(s),
            _ => None,
        })
    }
}
