//! Just enough CSS for a book's text: which elements are hidden or set as
//! blocks, bold or italic, how blocks are aligned, which are set in from
//! the left, which aren't indented or have space around them, and where
//! pages break. Sizes, fonts, colours
//! and the rest are left to the reader.
//!
//! Selectors are element names, `*`, `.class` and `#id`, joined by
//! descendant (space) and child (`>`) combinators. A selector with anything
//! else, such as `:first-child`, `[href]` or `+`, is dropped, so no rule
//! reaches further than the book meant. `@` rules are skipped, with what
//! `@media` and `@supports` hold. Of the rules for an element, the more
//! specific win, then the later; `!important` is ignored.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    None,
    Block,
    Inline,
}

/// What a rule or a `style` attribute sets, `None` where it says nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Declarations {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub align: Option<Align>,
    /// `text-indent`: false when it's zero or less.
    pub indent: Option<bool>,
    pub display: Option<Display>,
    pub break_before: Option<bool>,
    pub break_after: Option<bool>,
    /// Whether the top and bottom margins are half a line or more.
    pub margin_top: Option<bool>,
    pub margin_bottom: Option<bool>,
    /// Whether the left margin sets the block in: 1em or 4% or more.
    pub margin_left: Option<bool>,
}

impl Declarations {
    /// Reads declarations, `name: value; …`, as in a rule or a `style`
    /// attribute.
    pub fn parse(s: &str) -> Self {
        let mut d = Declarations::default();
        for decl in s.split(';') {
            if let Some((name, value)) = decl.split_once(':') {
                let value = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
                d.set(&name.trim().to_ascii_lowercase(), &value);
            }
        }
        d
    }

    fn set(&mut self, name: &str, v: &str) {
        let page_break = |v: &str| match v {
            "always" | "page" | "left" | "right" | "recto" | "verso" => Some(true),
            "auto" | "avoid" | "avoid-page" => Some(false),
            _ => None,
        };
        // Half a line or more.
        let wide = |v: &str| em(v).map(|e| e >= 0.5);
        let set_in = |v: &str| match v.strip_suffix('%') {
            Some(pct) => number(pct).map(|n| n >= 4.0),
            None => em(v).map(|e| e >= 1.0),
        };
        match name {
            "font-weight" => {
                self.bold = match v {
                    "bold" | "bolder" => Some(true),
                    "normal" | "lighter" => Some(false),
                    n => n.parse::<u32>().ok().map(|w| w >= 600),
                }
            }
            "font-style" => {
                self.italic = match v {
                    "italic" | "oblique" => Some(true),
                    "normal" => Some(false),
                    _ => None,
                }
            }
            "text-align" => {
                self.align = match v {
                    "left" | "start" => Some(Align::Left),
                    "right" | "end" => Some(Align::Right),
                    "center" => Some(Align::Center),
                    "justify" => Some(Align::Justify),
                    _ => None,
                }
            }
            "text-indent" => self.indent = number(v).map(|n| n > 0.0),
            "display" => {
                self.display = match v {
                    "none" => Some(Display::None),
                    "block" | "list-item" | "table" | "flex" | "grid" => Some(Display::Block),
                    "inline" | "inline-block" => Some(Display::Inline),
                    _ => self.display,
                }
            }
            "page-break-before" | "break-before" => self.break_before = page_break(v),
            "page-break-after" | "break-after" => self.break_after = page_break(v),
            "margin-top" => self.margin_top = wide(v),
            "margin-bottom" => self.margin_bottom = wide(v),
            "margin-left" => self.margin_left = set_in(v),
            "margin" => {
                let parts: Vec<&str> = v.split_whitespace().collect();
                let (top, bottom, left) = match parts.len() {
                    1 => (0, 0, 0),
                    2 => (0, 0, 1),
                    3 => (0, 2, 1),
                    4 => (0, 2, 3),
                    _ => return,
                };
                self.margin_top = wide(parts[top]);
                self.margin_bottom = wide(parts[bottom]);
                self.margin_left = set_in(parts[left]);
            }
            _ => {}
        }
    }

    /// Sets what `other` sets, over these.
    pub fn overlay(&mut self, o: &Declarations) {
        self.bold = o.bold.or(self.bold);
        self.italic = o.italic.or(self.italic);
        self.align = o.align.or(self.align);
        self.indent = o.indent.or(self.indent);
        self.display = o.display.or(self.display);
        self.break_before = o.break_before.or(self.break_before);
        self.break_after = o.break_after.or(self.break_after);
        self.margin_top = o.margin_top.or(self.margin_top);
        self.margin_bottom = o.margin_bottom.or(self.margin_bottom);
        self.margin_left = o.margin_left.or(self.margin_left);
    }
}

/// The number a length starts with, whatever its unit.
fn number(v: &str) -> Option<f32> {
    let end = v.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))).unwrap_or(v.len());
    v[..end].parse().ok()
}

/// A length in ems, for the units that convert: `em`, `rem`, `px` (16 to
/// the em), `pt` (12), `ex` and `ch` (half), and a bare 0.
fn em(v: &str) -> Option<f32> {
    let n = number(v)?;
    let unit = v.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
    match unit {
        "em" | "rem" => Some(n),
        "px" => Some(n / 16.0),
        "pt" => Some(n / 12.0),
        "ex" | "ch" => Some(n / 2.0),
        "" if n == 0.0 => Some(0.0),
        _ => None,
    }
}

/// An open element, as selectors see it.
pub trait Element {
    /// In lower case.
    fn name(&self) -> &str;
    fn id(&self) -> &str;
    /// The `class` attribute: names separated by spaces.
    fn class(&self) -> &str;
}

/// A compound selector, such as `p.note#first`.
#[derive(Debug, Default)]
struct Compound {
    /// None for any element.
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

impl Compound {
    fn parse(s: &str) -> Option<Compound> {
        let ident = |s: &str| -> usize { s.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')).unwrap_or(s.len()) };
        let mut c = Compound::default();
        let mut rest = s.strip_prefix('*').unwrap_or(s);
        if rest.len() == s.len() {
            let n = ident(rest);
            if n > 0 {
                c.tag = Some(rest[..n].to_ascii_lowercase());
                rest = &rest[n..];
            }
        }
        while let Some(first) = rest.chars().next() {
            let k = first.len_utf8();
            let n = ident(&rest[k..]);
            if n == 0 {
                return None;
            }
            let name = rest[k..k + n].to_string();
            match first {
                '.' => c.classes.push(name),
                '#' => c.id = Some(name),
                _ => return None,
            }
            rest = &rest[k + n..];
        }
        (c.tag.is_some() || c.id.is_some() || !c.classes.is_empty() || s == "*").then_some(c)
    }

    fn matches(&self, e: &impl Element) -> bool {
        self.tag.as_ref().is_none_or(|t| t == e.name())
            && self.id.as_ref().is_none_or(|id| id == e.id())
            && self.classes.iter().all(|c| e.class().split_whitespace().any(|k| k == c))
    }
}

/// A selector's compounds, left to right, each with whether the one
/// before it must be its parent (`>`) rather than any ancestor.
#[derive(Debug)]
struct Selector {
    parts: Vec<(Compound, bool)>,
    specificity: u32,
}

impl Selector {
    fn parse(s: &str) -> Option<Selector> {
        let mut parts = Vec::new();
        let mut child = false;
        for word in s.replace('>', " > ").split_whitespace() {
            if word == ">" {
                if parts.is_empty() || child {
                    return None;
                }
                child = true;
                continue;
            }
            parts.push((Compound::parse(word)?, child));
            child = false;
        }
        if parts.is_empty() || child {
            return None;
        }
        let specificity = parts
            .iter()
            .map(|(c, _)| c.id.is_some() as u32 * 10_000 + c.classes.len() as u32 * 100 + c.tag.is_some() as u32)
            .sum();
        Some(Selector { parts, specificity })
    }

    /// Whether it selects `path`'s last element; the others are its
    /// ancestors, outermost first.
    fn matches<E: Element>(&self, path: &[E]) -> bool {
        fn from<E: Element>(parts: &[(Compound, bool)], path: &[E]) -> bool {
            let Some(((compound, child), before)) = parts.split_last() else { return true };
            let Some((e, ancestors)) = path.split_last() else { return false };
            if !compound.matches(e) {
                return false;
            }
            if before.is_empty() {
                return true;
            }
            if *child {
                return from(before, ancestors);
            }
            // Any ancestor will do, nearest first.
            (1..=ancestors.len()).rev().any(|n| from(before, &ancestors[..n]))
        }
        from(&self.parts, path)
    }
}

#[derive(Debug)]
struct Rule {
    selector: Selector,
    decl: Declarations,
}

/// A stylesheet's rules that set something this module reads.
#[derive(Debug, Default)]
pub struct Stylesheet {
    rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn parse(css: &str) -> Stylesheet {
        let text = without_comments(css);
        let mut rules = Vec::new();
        let mut rest = text.as_str();
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '}');
            if rest.is_empty() {
                break;
            }
            let open = rest.find('{');
            if rest.starts_with('@') {
                // A statement ends at ';'; a block at its closing brace.
                match (rest.find(';'), open) {
                    (Some(semi), open) if open.is_none_or(|o| semi < o) => rest = &rest[semi + 1..],
                    (_, Some(open)) => rest = after_block(rest, open),
                    _ => break,
                }
                continue;
            }
            let Some(open) = open else { break };
            let Some(len) = rest[open..].find('}') else { break };
            let decl = Declarations::parse(&rest[open + 1..open + len]);
            if decl != Declarations::default() {
                for s in rest[..open].split(',') {
                    if let Some(selector) = Selector::parse(s.trim()) {
                        rules.push(Rule { selector, decl });
                    }
                }
            }
            rest = &rest[open + len + 1..];
        }
        Stylesheet { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// `css` without its comments.
fn without_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..].find("*/").map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// What follows the block that opens at byte `open` of `s`.
fn after_block(s: &str, open: usize) -> &str {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &s[open + i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// What the rules of `sheets`, in order, set for `path`'s last element; the
/// others are its ancestors, outermost first.
pub fn cascade<E: Element>(sheets: &[&Stylesheet], path: &[E]) -> Declarations {
    // (specificity, sheet, rule), in the sheets' order.
    let mut found: Vec<(u32, usize, usize)> = Vec::new();
    for (s, sheet) in sheets.iter().enumerate() {
        for (r, rule) in sheet.rules.iter().enumerate() {
            if rule.selector.matches(path) {
                // Kept in order of specificity, the later after the earlier.
                let at = found.partition_point(|&(sp, _, _)| sp <= rule.selector.specificity);
                found.insert(at, (rule.selector.specificity, s, r));
            }
        }
    }
    let mut d = Declarations::default();
    for (_, s, r) in found {
        d.overlay(&sheets[s].rules[r].decl);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    struct E(&'static str, &'static str, &'static str);

    impl Element for E {
        fn name(&self) -> &str {
            self.0
        }
        fn id(&self) -> &str {
            self.1
        }
        fn class(&self) -> &str {
            self.2
        }
    }

    #[test]
    fn declarations() {
        let d = Declarations::parse("font-weight: 700; FONT-STYLE: Italic ; text-align:center!important; text-indent: 0; margin: 0 0 1em");
        assert_eq!((d.bold, d.italic, d.align, d.indent), (Some(true), Some(true), Some(Align::Center), Some(false)));
        assert_eq!((d.margin_top, d.margin_bottom), (Some(false), Some(true)));
        let d = Declarations::parse("text-indent: 4%; margin-top: 0.2em; page-break-before: always; display: none; color: red");
        assert_eq!((d.indent, d.margin_top, d.break_before, d.display), (Some(true), Some(false), Some(true), Some(Display::None)));
        assert_eq!(Declarations::parse("text-indent: -1em").indent, Some(false));
        assert_eq!(Declarations::parse("margin: 12px auto").margin_top, Some(true));
        let d = Declarations::parse("margin: 1em 0");
        assert_eq!((d.margin_top, d.margin_left), (Some(true), Some(false)));
        assert_eq!(Declarations::parse("margin: 0 10% 0 5%").margin_left, Some(true));
        assert_eq!(Declarations::parse("margin-left: 2.5em").margin_left, Some(true));
        assert_eq!(Declarations::parse("margin-left: auto").margin_left, None);
        assert_eq!(Declarations::parse("color: red; float: left"), Declarations::default());
    }

    #[test]
    fn rules_and_cascade() {
        let sheet = Stylesheet::parse(
            "@charset \"utf-8\";\n/* note { font-weight: bold } */\n\
             p { text-indent: 1em } .c, h2 { text-align: center }\n\
             @media screen { p { display: none } }\n@supports (display: flex) { .c { display: none } }\n\
             div.note p { font-style: italic } div > .x { font-weight: bold }\n\
             p:first-child { display: none } a[href] { display: none } h2 + p { display: none }\n\
             .epigraph { font-style: italic } .epigraph i { font-style: normal }\n\
             #top { text-align: right } p.c { text-align: left }",
        );
        // The rules kept: p, .c, h2, div.note p, div > .x, .epigraph, .epigraph i, #top, p.c.
        assert_eq!(sheet.rules.len(), 9);
        let get = |path: &[E]| cascade(&[&sheet], path);
        let p = get(&[E("body", "", ""), E("p", "", "")]);
        assert_eq!((p.indent, p.display, p.align), (Some(true), None, None));
        assert_eq!(get(&[E("p", "", "c")]).align, Some(Align::Left));
        assert_eq!(get(&[E("div", "", "c")]).align, Some(Align::Center));
        assert_eq!(get(&[E("p", "top", "c")]).align, Some(Align::Right));
        assert_eq!(get(&[E("div", "", "note wide"), E("blockquote", "", ""), E("p", "", "")]).italic, Some(true));
        assert_eq!(get(&[E("div", "", "note"), E("span", "", "x")]).bold, Some(true));
        assert_eq!(get(&[E("div", "", ""), E("p", "", ""), E("span", "", "x")]).bold, None);
        assert_eq!(get(&[E("section", "", "epigraph"), E("p", "", ""), E("i", "", "")]).italic, Some(false));
    }

    #[test]
    fn broken_css_is_survived() {
        assert!(Stylesheet::parse("p { text-align: center").is_empty());
        assert!(Stylesheet::parse("}} @media print { p { x: y } ").is_empty());
        let sheet = Stylesheet::parse("p { text-align: } .a { font-style: italic } /* unclosed");
        assert_eq!(sheet.rules.len(), 1);
        // A stray character spoils its rule only.
        assert_eq!(Stylesheet::parse("p→ { display: none } ; .b { display: none } .c { display: none }").rules.len(), 1);
    }
}
