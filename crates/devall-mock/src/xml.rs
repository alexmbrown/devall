//! XML support for `equalToXml`, `matchesXPath` and the `xPath` template helper.
//!
//! Comparison is semantic: element and attribute names are compared by namespace URI + local name,
//! attribute order and whitespace-only text are ignored, text is trimmed, comments and processing
//! instructions are ignored. With placeholders enabled, `${xmlunit.ignore}` matches any text or
//! attribute value. XPath is XPath 1.0 (via `sxd-xpath`); DTDs / entity declarations are not
//! supported by the parser.

use std::collections::BTreeMap;

use sxd_document::dom::{ChildOfElement, ChildOfRoot, Element};
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, Factory, Value as XValue};

#[derive(Debug, Clone, PartialEq)]
pub enum XNode {
    Element { ns: String, name: String, attrs: BTreeMap<(String, String), String>, children: Vec<XNode> },
    Text(String),
}

fn strip_prolog(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

pub fn parse_tree(text: &str) -> Result<XNode, String> {
    let package = sxd_document::parser::parse(strip_prolog(text)).map_err(|e| format!("invalid XML: {e:?}"))?;
    let doc = package.as_document();
    let root = doc
        .root()
        .children()
        .into_iter()
        .find_map(|c| match c {
            ChildOfRoot::Element(e) => Some(e),
            _ => None,
        })
        .ok_or("XML has no root element")?;
    Ok(convert(root))
}

fn convert(e: Element<'_>) -> XNode {
    let q = e.name();
    let mut attrs = BTreeMap::new();
    for a in e.attributes() {
        let n = a.name();
        attrs.insert((n.namespace_uri().unwrap_or("").to_string(), n.local_part().to_string()), a.value().to_string());
    }
    let mut children = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, children: &mut Vec<XNode>| {
        let t = text.trim();
        if !t.is_empty() {
            children.push(XNode::Text(t.to_string()));
        }
        text.clear();
    };
    for c in e.children() {
        match c {
            ChildOfElement::Element(child) => {
                flush(&mut text, &mut children);
                children.push(convert(child));
            }
            ChildOfElement::Text(t) => text.push_str(t.text()),
            _ => {}
        }
    }
    flush(&mut text, &mut children);
    XNode::Element { ns: q.namespace_uri().unwrap_or("").to_string(), name: q.local_part().to_string(), attrs, children }
}

const IGNORE: &str = "${xmlunit.ignore}";

pub fn xml_equal(expected: &XNode, actual: &XNode, placeholders: bool) -> bool {
    match (expected, actual) {
        (XNode::Text(e), XNode::Text(a)) => (placeholders && e == IGNORE) || e == a,
        (XNode::Element { ns: en, name: ename, attrs: ea, children: ec }, XNode::Element { ns: an, name: aname, attrs: aa, children: ac }) => {
            if en != an || ename != aname || ea.len() != aa.len() || ec.len() != ac.len() {
                return false;
            }
            let attrs_ok = ea.iter().all(|(k, v)| aa.get(k).is_some_and(|av| (placeholders && v == IGNORE) || av == v));
            attrs_ok && ec.iter().zip(ac).all(|(e, a)| xml_equal(e, a, placeholders))
        }
        _ => false,
    }
}

/// Counts differing nodes for near-miss distances (0..=1).
pub fn xml_distance(expected: &XNode, actual: &XNode) -> f64 {
    fn count(n: &XNode) -> usize {
        match n {
            XNode::Text(_) => 1,
            XNode::Element { attrs, children, .. } => 1 + attrs.len() + children.iter().map(count).sum::<usize>(),
        }
    }
    fn diff(e: &XNode, a: &XNode) -> usize {
        match (e, a) {
            (XNode::Text(x), XNode::Text(y)) => usize::from(x != y),
            (XNode::Element { ns: en, name: ename, attrs: ea, children: ec }, XNode::Element { ns: an, name: aname, attrs: aa, children: ac }) => {
                if en != an || ename != aname {
                    return count(e);
                }
                let mut d = ea.iter().filter(|(k, v)| aa.get(*k) != Some(*v)).count();
                d += aa.keys().filter(|k| !ea.contains_key(*k)).count();
                for (i, ce) in ec.iter().enumerate() {
                    d += ac.get(i).map_or_else(|| count(ce), |ca| diff(ce, ca));
                }
                d + ac.len().saturating_sub(ec.len())
            }
            _ => count(e),
        }
    }
    let total = count(expected).max(1);
    (diff(expected, actual) as f64 / total as f64).min(1.0)
}

/// One XPath result value as WireMock exposes it: text content, and the XML rendering for
/// element nodes (used by `equalToXml` sub-matchers).
#[derive(Debug, Clone, PartialEq)]
pub struct XPathItem {
    pub text: String,
    pub xml: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum XPathResult {
    Nodes(Vec<XPathItem>),
    Bool(bool),
    Number(f64),
    Str(String),
}

impl XPathResult {
    /// Values a sub-matcher is tried against (numbers in Java style: `3` not `3.0`).
    pub fn values(&self) -> Vec<XPathItem> {
        match self {
            XPathResult::Nodes(n) => n.clone(),
            XPathResult::Bool(b) => vec![XPathItem { text: b.to_string(), xml: None }],
            XPathResult::Number(n) => vec![XPathItem { text: fmt_num(*n), xml: None }],
            XPathResult::Str(s) => vec![XPathItem { text: s.clone(), xml: None }],
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            XPathResult::Nodes(n) => !n.is_empty(),
            XPathResult::Bool(b) => *b,
            XPathResult::Number(n) => *n != 0.0 && !n.is_nan(),
            XPathResult::Str(s) => !s.is_empty(),
        }
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() }
}

/// Validates an XPath expression (used when a stub is compiled).
pub fn check_xpath(expr: &str) -> Result<(), String> {
    match Factory::new().build(expr) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err("empty XPath expression".into()),
        Err(e) => Err(format!("invalid XPath {expr:?}: {e:?}")),
    }
}

pub fn eval_xpath(xml: &str, expr: &str, namespaces: &[(String, String)]) -> Result<XPathResult, String> {
    let package = sxd_document::parser::parse(strip_prolog(xml)).map_err(|e| format!("invalid XML: {e:?}"))?;
    let doc = package.as_document();
    let xpath = Factory::new().build(expr).map_err(|e| format!("invalid XPath: {e:?}"))?.ok_or("empty XPath expression")?;
    let mut ctx = Context::new();
    for (prefix, uri) in namespaces {
        ctx.set_namespace(prefix, uri);
    }
    let value = xpath.evaluate(&ctx, doc.root()).map_err(|e| format!("XPath error: {e:?}"))?;
    Ok(match value {
        XValue::Boolean(b) => XPathResult::Bool(b),
        XValue::Number(n) => XPathResult::Number(n),
        XValue::String(s) => XPathResult::Str(s),
        XValue::Nodeset(ns) => XPathResult::Nodes(
            ns.document_order()
                .into_iter()
                .map(|n| XPathItem {
                    text: n.string_value(),
                    xml: match n {
                        Node::Element(e) => Some(render_element(e)),
                        _ => None,
                    },
                })
                .collect(),
        ),
    })
}

fn escape(text: &str, attr: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn qualified(prefix: Option<&str>, local: &str) -> String {
    match prefix {
        Some(p) if !p.is_empty() => format!("{p}:{local}"),
        _ => local.to_string(),
    }
}

/// Serialises an element subtree (namespace declarations for used prefixes included).
pub fn render_element(e: Element<'_>) -> String {
    fn walk(e: Element<'_>, out: &mut String, top: bool) {
        let q = e.name();
        let name = qualified(e.preferred_prefix(), q.local_part());
        out.push('<');
        out.push_str(&name);
        if top && let Some(uri) = q.namespace_uri() {
            match e.preferred_prefix() {
                Some(p) => out.push_str(&format!(" xmlns:{p}=\"{}\"", escape(uri, true))),
                None => out.push_str(&format!(" xmlns=\"{}\"", escape(uri, true))),
            }
        }
        for a in e.attributes() {
            let an = a.name();
            out.push_str(&format!(" {}=\"{}\"", qualified(a.preferred_prefix(), an.local_part()), escape(a.value(), true)));
        }
        let children = e.children();
        if children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        for c in children {
            match c {
                ChildOfElement::Element(child) => walk(child, out, false),
                ChildOfElement::Text(t) => out.push_str(&escape(t.text(), false)),
                _ => {}
            }
        }
        out.push_str(&format!("</{name}>"));
    }
    let mut out = String::new();
    walk(e, &mut out, true);
    out
}

/// Pretty-prints XML with two-space indentation (`formatXml` helper).
pub fn pretty(xml: &str) -> Result<String, String> {
    fn walk(n: &XNode, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        match n {
            XNode::Text(t) => {
                out.push_str(&pad);
                out.push_str(&escape(t, false));
                out.push('\n');
            }
            XNode::Element { name, attrs, children, .. } => {
                out.push_str(&pad);
                out.push('<');
                out.push_str(name);
                for ((_, k), v) in attrs {
                    out.push_str(&format!(" {k}=\"{}\"", escape(v, true)));
                }
                match children.as_slice() {
                    [] => out.push_str("/>\n"),
                    [XNode::Text(t)] => out.push_str(&format!(">{}</{name}>\n", escape(t, false))),
                    _ => {
                        out.push_str(">\n");
                        for c in children {
                            walk(c, depth + 1, out);
                        }
                        out.push_str(&format!("{pad}</{name}>\n"));
                    }
                }
            }
        }
    }
    let tree = parse_tree(xml)?;
    let mut out = String::new();
    walk(&tree, 0, &mut out);
    Ok(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_equality() {
        let a = parse_tree("<a x=\"1\" y=\"2\">\n  <b>hi</b>\n  <!-- c -->\n</a>").unwrap();
        let b = parse_tree("<?xml version=\"1.0\"?><a y=\"2\" x=\"1\"><b> hi </b></a>").unwrap();
        assert!(xml_equal(&a, &b, false));
        let c = parse_tree("<a x=\"1\" y=\"2\"><b>bye</b></a>").unwrap();
        assert!(!xml_equal(&a, &c, false));
        assert!(xml_distance(&a, &c) > 0.0);
        let p = parse_tree("<a x=\"1\" y=\"${xmlunit.ignore}\"><b>${xmlunit.ignore}</b></a>").unwrap();
        assert!(xml_equal(&p, &c, true));
        let ns1 = parse_tree("<p:a xmlns:p=\"urn:x\"><p:b>1</p:b></p:a>").unwrap();
        let ns2 = parse_tree("<a xmlns=\"urn:x\"><b>1</b></a>").unwrap();
        assert!(xml_equal(&ns1, &ns2, false));
        assert!(parse_tree("<a>").is_err());
    }

    #[test]
    fn xpath() {
        let xml = "<todo-list><todo-item id=\"1\">wash</todo-item><todo-item id=\"2\">dry</todo-item></todo-list>";
        let r = eval_xpath(xml, "//todo-item", &[]).unwrap();
        assert_eq!(r.values().len(), 2);
        assert_eq!(r.values()[0].text, "wash");
        assert_eq!(r.values()[0].xml.as_deref(), Some("<todo-item id=\"1\">wash</todo-item>"));
        assert_eq!(eval_xpath(xml, "count(//todo-item)", &[]).unwrap(), XPathResult::Number(2.0));
        assert!(eval_xpath(xml, "//todo-item[@id='2']", &[]).unwrap().is_truthy());
        assert!(!eval_xpath(xml, "//nothing", &[]).unwrap().is_truthy());
        let nsxml = "<s:Envelope xmlns:s=\"urn:soap\"><s:Body>x</s:Body></s:Envelope>";
        let r = eval_xpath(nsxml, "/e:Envelope/e:Body/text()", &[("e".into(), "urn:soap".into())]).unwrap();
        assert_eq!(r.values()[0].text, "x");
        assert!(check_xpath("//[").is_err());
        assert!(pretty("<a><b>1</b></a>").unwrap().contains("  <b>1</b>"));
    }
}
