//! DTS v1 parser (input: preprocessed text).

use crate::{
    tree::{Cell, Node, Property, Tree, Value},
    Error,
};

/// A deferred `&label { … }` / `/delete-node/ &label;` at the top level.
enum TopItem {
    Root(Node),
    Ref(String, Node),
    DeleteRef(String),
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b",._+*#?@-".contains(&c)
}

impl<'a> P<'a> {
    fn err(&self, msg: &str) -> Error {
        let line = self.s[..self.i.min(self.s.len())].iter().filter(|&&c| c == b'\n').count() + 1;
        let ctx: String = String::from_utf8_lossy(&self.s[self.i.min(self.s.len())..(self.i + 30).min(self.s.len())]).into();
        Error::new(format!("line {line}: {msg} (near {:?})", ctx))
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &str) -> Result<(), Error> {
        if self.eat(t) { Ok(()) } else { Err(self.err(&format!("expected `{t}`"))) }
    }
    fn name(&mut self) -> Result<String, Error> {
        self.ws();
        let s = self.i;
        while self.i < self.s.len() && is_name_char(self.s[self.i]) {
            self.i += 1;
        }
        if s == self.i {
            return Err(self.err("expected a name"));
        }
        Ok(String::from_utf8_lossy(&self.s[s..self.i]).into())
    }
    /// `label:` prefixes.
    fn labels(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        loop {
            self.ws();
            let save = self.i;
            let s = self.i;
            while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                self.i += 1;
            }
            if self.i > s && self.s.get(self.i) == Some(&b':') {
                out.push(String::from_utf8_lossy(&self.s[s..self.i]).into());
                self.i += 1;
            } else {
                self.i = save;
                return out;
            }
        }
    }
    fn reference(&mut self) -> Result<String, Error> {
        self.expect("&")?;
        if self.s.get(self.i) == Some(&b'{') {
            let s = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'}' {
                self.i += 1;
            }
            self.i += 1;
            return Ok(String::from_utf8_lossy(&self.s[s..self.i]).into());
        }
        let s = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
            self.i += 1;
        }
        if s == self.i {
            return Err(self.err("expected a label after `&`"));
        }
        Ok(String::from_utf8_lossy(&self.s[s..self.i]).into())
    }

    fn string(&mut self) -> Result<String, Error> {
        self.expect("\"")?;
        let mut out = String::new();
        while self.i < self.s.len() && self.s[self.i] != b'"' {
            let c = self.s[self.i];
            if c == b'\\' && self.i + 1 < self.s.len() {
                self.i += 1;
                out.push(match self.s[self.i] {
                    b'n' => '\n',
                    b't' => '\t',
                    b'0' => '\0',
                    o => o as char,
                });
            } else {
                out.push(c as char);
            }
            self.i += 1;
        }
        self.expect("\"")?;
        Ok(out)
    }

    // ---- expressions (dtc semantics: 64-bit arithmetic, truncated to the cell) ----
    fn primary(&mut self) -> Result<i64, Error> {
        self.ws();
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let v = self.expr()?;
                self.expect(")")?;
                Ok(v)
            }
            Some(b'-') => {
                self.i += 1;
                Ok(self.primary()?.wrapping_neg())
            }
            Some(b'~') => {
                self.i += 1;
                Ok(!self.primary()?)
            }
            Some(b'!') => {
                self.i += 1;
                Ok((self.primary()? == 0) as i64)
            }
            Some(b'\'') => {
                self.i += 1;
                let c = self.s[self.i] as i64;
                self.i += 1;
                self.expect("'")?;
                Ok(c)
            }
            Some(c) if c.is_ascii_digit() => {
                let s = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric()) {
                    self.i += 1;
                }
                let t = String::from_utf8_lossy(&self.s[s..self.i]).to_string();
                let t = t.trim_end_matches(['U', 'L', 'u', 'l']);
                let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    i64::from_str_radix(h, 16)
                } else if t.len() > 1 && t.starts_with('0') {
                    i64::from_str_radix(&t[1..], 8)
                } else {
                    t.parse::<i64>()
                };
                v.map_err(|_| self.err(&format!("bad number {t}")))
            }
            _ => Err(self.err("expected a number or expression")),
        }
    }
    fn binop(&mut self) -> Option<(&'static str, u8)> {
        self.ws();
        const OPS: [(&str, u8); 18] = [
            ("||", 1), ("&&", 2), ("==", 6), ("!=", 6), ("<=", 7), (">=", 7), ("<<", 8), (">>", 8),
            ("|", 3), ("^", 4), ("&", 5), ("<", 7), (">", 7), ("+", 9), ("-", 9), ("*", 10), ("/", 10), ("%", 10),
        ];
        OPS.iter().find(|(op, _)| self.s[self.i..].starts_with(op.as_bytes())).copied()
    }
    fn expr_bp(&mut self, min: u8) -> Result<i64, Error> {
        let mut lhs = self.primary()?;
        while let Some((op, bp)) = self.binop() {
            if bp < min {
                break;
            }
            self.i += op.len();
            let rhs = self.expr_bp(bp + 1)?;
            lhs = match op {
                "||" => ((lhs != 0) || (rhs != 0)) as i64,
                "&&" => ((lhs != 0) && (rhs != 0)) as i64,
                "==" => (lhs == rhs) as i64,
                "!=" => (lhs != rhs) as i64,
                "<=" => (lhs <= rhs) as i64,
                ">=" => (lhs >= rhs) as i64,
                "<" => (lhs < rhs) as i64,
                ">" => (lhs > rhs) as i64,
                "<<" => lhs.wrapping_shl(rhs as u32),
                ">>" => lhs.wrapping_shr(rhs as u32),
                "|" => lhs | rhs,
                "^" => lhs ^ rhs,
                "&" => lhs & rhs,
                "+" => lhs.wrapping_add(rhs),
                "-" => lhs.wrapping_sub(rhs),
                "*" => lhs.wrapping_mul(rhs),
                "/" => lhs.checked_div(rhs).ok_or_else(|| self.err("division by zero"))?,
                "%" => lhs.checked_rem(rhs).ok_or_else(|| self.err("division by zero"))?,
                _ => unreachable!(),
            };
        }
        Ok(lhs)
    }
    fn expr(&mut self) -> Result<i64, Error> {
        self.expr_bp(0)
    }

    fn cells(&mut self) -> Result<Vec<Cell>, Error> {
        self.expect("<")?;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                Some(b'>') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'&') => out.push(Cell::Ref(self.reference()?)),
                Some(b'(') | Some(b'-') | Some(b'~') | Some(b'\'') | Some(b'0'..=b'9') => {
                    out.push(Cell::Num(self.primary()? as u32))
                }
                _ => return Err(self.err("bad cell")),
            }
        }
    }

    fn bytes(&mut self) -> Result<Vec<u8>, Error> {
        self.expect("[")?;
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.eat("]") {
                return Ok(out);
            }
            let hex = |c: u8| (c as char).to_digit(16);
            let (a, b) = (self.s.get(self.i).copied(), self.s.get(self.i + 1).copied());
            match (a.and_then(hex), b.and_then(hex)) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    self.i += 2;
                }
                _ => return Err(self.err("bad byte string")),
            }
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek() {
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'<') => Ok(Value::Cells(self.cells()?)),
            Some(b'[') => Ok(Value::Bytes(self.bytes()?)),
            Some(b'&') => Ok(Value::PathRef(self.reference()?)),
            _ => Err(self.err("expected a property value")),
        }
    }

    /// Body of a node, after `{`, up to and including `};`.
    fn body(&mut self, node: &mut Node) -> Result<(), Error> {
        loop {
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    self.expect(";")?;
                    return Ok(());
                }
                None => return Err(self.err("unexpected end of input in node")),
                _ => {}
            }
            if self.eat("/delete-node/") {
                let name = self.name()?;
                self.expect(";")?;
                node.children.retain(|c| c.name != name && c.base_name() != name);
                continue;
            }
            if self.eat("/delete-property/") {
                let name = self.name()?;
                self.expect(";")?;
                node.props.retain(|p| p.name != name);
                continue;
            }
            let labels = self.labels();
            let name = self.name()?;
            match self.peek() {
                Some(b'{') => {
                    self.i += 1;
                    let mut child = Node { name, labels, ..Default::default() };
                    self.body(&mut child)?;
                    match node.children.iter_mut().find(|c| c.name == child.name) {
                        Some(c) => c.merge(child),
                        None => node.children.push(child),
                    }
                }
                Some(b'=') => {
                    self.i += 1;
                    let mut value = vec![self.value()?];
                    while self.eat(",") {
                        value.push(self.value()?);
                    }
                    self.expect(";")?;
                    set_prop(node, Property { name, value });
                }
                Some(b';') => {
                    self.i += 1;
                    set_prop(node, Property { name, value: Vec::new() });
                }
                _ => return Err(self.err("expected `{`, `=` or `;`")),
            }
        }
    }

    fn top(&mut self) -> Result<Vec<TopItem>, Error> {
        let mut items = Vec::new();
        loop {
            if self.peek().is_none() {
                return Ok(items);
            }
            if self.eat("/dts-v1/") || self.eat("/plugin/") {
                self.expect(";")?;
                continue;
            }
            if self.eat("/memreserve/") {
                while self.peek() != Some(b';') {
                    self.i += 1;
                }
                self.i += 1;
                continue;
            }
            if self.eat("/delete-node/") {
                let r = self.reference()?;
                self.expect(";")?;
                items.push(TopItem::DeleteRef(r));
                continue;
            }
            let labels = self.labels();
            if self.peek() == Some(b'/') {
                self.i += 1;
                self.expect("{")?;
                let mut n = Node { name: "/".into(), labels, ..Default::default() };
                self.body(&mut n)?;
                items.push(TopItem::Root(n));
            } else if self.peek() == Some(b'&') {
                let r = self.reference()?;
                self.expect("{")?;
                let mut n = Node { labels, ..Default::default() };
                self.body(&mut n)?;
                items.push(TopItem::Ref(r, n));
            } else {
                return Err(self.err("expected `/`, `&label` or a directive at top level"));
            }
        }
    }
}

fn set_prop(node: &mut Node, p: Property) {
    match node.props.iter_mut().find(|q| q.name == p.name) {
        Some(q) => *q = p,
        None => node.props.push(p),
    }
}

/// Parse preprocessed DTS text into a merged tree.
pub fn parse(src: &str) -> Result<Tree, Error> {
    let mut p = P { s: src.as_bytes(), i: 0 };
    let items = p.top()?;
    let mut tree = Tree { root: Node { name: "/".into(), ..Default::default() } };
    for item in items {
        match item {
            TopItem::Root(n) => tree.root.merge(n),
            TopItem::Ref(r, mut n) => {
                let path = tree.resolve(&r).ok_or_else(|| Error::new(format!("unresolved reference &{r}")))?;
                let target = tree.node_mut(&path).unwrap();
                n.name = target.name.clone();
                target.merge(n);
            }
            TopItem::DeleteRef(r) => {
                let path = tree.resolve(&r).ok_or_else(|| Error::new(format!("unresolved reference &{r}")))?;
                let parent = Tree::parent_path(&path).ok_or_else(|| Error::new("cannot delete the root"))?;
                let name = path.rsplit('/').next().unwrap().to_string();
                tree.node_mut(&parent).unwrap().children.retain(|c| c.name != name);
            }
        }
    }
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
/dts-v1/;
/ {
    #address-cells = <1>;
    model = "Test";
    compatible = "a,b", "c,d";
    soc {
        gpio: gpio@1000 { gpio-controller; #gpio-cells = <2>; };
        i2c0: i2c@2000 { status = "disabled"; #address-cells = <1>; #size-cells = <0>; };
    };
    chosen { kumiki,display = &lcd; };
};
&i2c0 {
    status = "okay";
    clock-frequency = <(400 * 1000)>;
    lcd: display@3c { compatible = "x,y"; reg = <0x3c>; reset-gpios = <&gpio 3 1>; offset = <(-29) 0>; id = [00 91 07]; };
};
/ { soc { /delete-node/ gpio@9999; }; };
"#;

    #[test]
    fn parses_merges_and_resolves() {
        let t = parse(SRC).unwrap();
        assert_eq!(t.root.prop("model").unwrap().str(), Some("Test"));
        assert_eq!(t.root.compatible(), vec!["a,b", "c,d"]);
        let i2c = t.by_ref("i2c0").unwrap();
        assert!(i2c.is_okay());
        assert_eq!(i2c.prop("clock-frequency").unwrap().u32(), Some(400_000));
        let lcd = t.by_ref("lcd").unwrap();
        assert_eq!(lcd.unit(), Some("3c"));
        assert_eq!(lcd.prop("reset-gpios").unwrap().cells(), vec![Cell::Ref("gpio".into()), Cell::Num(3), Cell::Num(1)]);
        assert_eq!(lcd.prop("offset").unwrap().u32s()[0] as i32, -29);
        assert_eq!(lcd.prop("id").unwrap().bytes(), vec![0x00, 0x91, 0x07]);
        assert_eq!(t.chosen("kumiki,display").as_deref(), Some("/soc/i2c@2000/display@3c"));
        assert!(t.by_ref("gpio").unwrap().has("gpio-controller"));
    }

    #[test]
    fn delete_node_by_ref() {
        let t = parse("/dts-v1/;\n/ { a: x { }; y { }; };\n/delete-node/ &a;\n").unwrap();
        assert!(t.node("/x").is_none());
        assert!(t.node("/y").is_some());
    }

    #[test]
    fn reports_line_numbers() {
        let e = parse("/dts-v1/;\n/ {\n  bad = <1 2;\n};\n").unwrap_err();
        assert!(e.to_string().contains("line 3"), "{e}");
    }
}
