//! Random strings matching a regex, via `rand_regex`, with two adjustments that make generated
//! test data look sane: anchors/word boundaries are dropped (JSON Schema `pattern`s are usually
//! `^…$`), and Unicode classes are narrowed to printable ASCII when they overlap it (so `\d` is
//! `[0-9]` and `.` doesn't produce Devanagari).

use rand::RngExt;
use regex_syntax::ParserBuilder;
use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, Hir, HirKind};

use crate::error::{Result, invalid};
use crate::fakers::DRng;

/// Extra repetitions allowed for `*`, `+` and `{n,}`.
const MAX_REPEAT: u32 = 8;

#[derive(Debug, Clone)]
pub struct RegexGen(rand_regex::Regex);

impl RegexGen {
    pub fn compile(pattern: &str) -> Result<Self> {
        let hir = ParserBuilder::new().build().parse(pattern).map_err(|e| invalid(format!("invalid regex {pattern:?}: {e}")))?;
        let generator = rand_regex::Regex::with_hir(asciify(hir), MAX_REPEAT).map_err(|e| invalid(format!("unsupported regex {pattern:?}: {e}")))?;
        Ok(Self(generator))
    }

    pub fn sample(&self, rng: &mut DRng) -> String {
        let bytes: Vec<u8> = rng.sample(&self.0);
        match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        }
    }
}

fn asciify(h: Hir) -> Hir {
    match h.into_kind() {
        HirKind::Look(_) => Hir::empty(),
        HirKind::Class(Class::Unicode(c)) => {
            let mut narrowed = c.clone();
            narrowed.intersect(&ClassUnicode::new([ClassUnicodeRange::new(' ', '~')]));
            Hir::class(Class::Unicode(if narrowed.ranges().is_empty() { c } else { narrowed }))
        }
        HirKind::Class(c) => Hir::class(c),
        HirKind::Repetition(mut r) => {
            r.sub = Box::new(asciify(*r.sub));
            Hir::repetition(r)
        }
        HirKind::Capture(mut c) => {
            c.sub = Box::new(asciify(*c.sub));
            Hir::capture(c)
        }
        HirKind::Concat(v) => Hir::concat(v.into_iter().map(asciify).collect()),
        HirKind::Alternation(v) => Hir::alternation(v.into_iter().map(asciify).collect()),
        HirKind::Literal(l) => Hir::literal(l.0),
        HirKind::Empty => Hir::empty(),
    }
}
