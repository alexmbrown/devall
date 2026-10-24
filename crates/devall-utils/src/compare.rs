//! String compare: are two strings *really* equal? Equality under common normalizations (case,
//! whitespace, line endings, Unicode NFC/NFKC, invisible characters, look-alikes), lengths in
//! every unit that matters, the first difference with context, a character-level diff and every
//! invisible / suspicious character with its code point.

use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use similar::{Algorithm, ChangeTag, TextDiff};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::unicode::{confusable, control_name, is_bidi_control, is_invisible, is_unusual_space, script};

/// Character diffs are skipped above this many characters per side (the rest still works).
const MAX_DIFF_CHARS: usize = 50_000;
const DIFF_TIMEOUT: Duration = Duration::from_secs(2);
/// Findings reported per side.
const MAX_FINDINGS: usize = 200;
/// Characters of context around the first difference.
const CONTEXT: usize = 16;

fn sha256(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Unicode-aware whitespace collapse: any run of whitespace (NBSP, tabs, CR/LF…) → one space, trimmed.
fn collapse_ws(s: &str) -> String {
    s.split(char::is_whitespace).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

fn unify_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn strip_invisible(s: &str) -> String {
    s.chars().filter(|c| !is_invisible(*c) && !is_bidi_control(*c)).collect()
}

/// Maps look-alike characters (Cyrillic `а`, Greek `ο`, fullwidth, typographic dashes/quotes…)
/// to their Latin/ASCII counterparts.
fn skeleton(s: &str) -> String {
    s.nfkc().map(|c| confusable(c).unwrap_or(c)).collect()
}

/// Everything at once: NFKC, look-alikes folded, invisible characters dropped, case folded,
/// whitespace collapsed.
fn loose(s: &str) -> String {
    collapse_ws(&strip_invisible(&skeleton(s)).to_lowercase())
}

fn line_endings(s: &str) -> &'static str {
    let crlf = s.matches("\r\n").count();
    let cr = s.matches('\r').count() - crlf;
    let lf = s.matches('\n').count() - crlf;
    match (crlf > 0, cr > 0, lf > 0) {
        (false, false, false) => "none",
        (true, false, false) => "CRLF",
        (false, true, false) => "CR",
        (false, false, true) => "LF",
        _ => "mixed",
    }
}

fn char_name(c: char) -> Option<String> {
    control_name(c).map(str::to_string).or_else(|| unicode_names2::name(c).map(|n| n.to_string()))
}

fn code_point(c: char) -> String {
    format!("U+{:04X}", c as u32)
}

fn describe(c: char) -> Value {
    json!({ "char": c.to_string(), "codePoint": code_point(c), "name": char_name(c), "script": script(c) })
}

fn side_stats(s: &str) -> Value {
    let mut ws = [0usize; 5]; // spaces, tabs, CR, LF, other whitespace
    for c in s.chars() {
        match c {
            ' ' => ws[0] += 1,
            '\t' => ws[1] += 1,
            '\r' => ws[2] += 1,
            '\n' => ws[3] += 1,
            c if c.is_whitespace() => ws[4] += 1,
            _ => {}
        }
    }
    json!({
        "chars": s.chars().count(),
        "graphemes": s.graphemes(true).count(),
        "utf8Bytes": s.len(),
        "utf16Units": s.encode_utf16().count(),
        "lines": if s.is_empty() { 0 } else { unify_newlines(s).split('\n').count() },
        "lineEndings": line_endings(s),
        "bom": s.starts_with('\u{feff}'),
        "trailingNewline": s.ends_with('\n') || s.ends_with('\r'),
        "leadingWhitespace": s.starts_with(char::is_whitespace),
        "trailingWhitespace": s.ends_with(char::is_whitespace),
        "nonAscii": s.chars().filter(|c| !c.is_ascii()).count(),
        "isNfc": unicode_normalization::is_nfc(s),
        "isNfkc": unicode_normalization::is_nfkc(s),
        "whitespace": { "spaces": ws[0], "tabs": ws[1], "cr": ws[2], "lf": ws[3], "other": ws[4] },
        "sha256": sha256(s),
    })
}

/// Invisible, bidi, control, unusual-whitespace and look-alike characters of one side.
fn findings(side: &str, s: &str, out: &mut Vec<Value>) -> usize {
    let mut n = 0usize;
    for (i, c) in s.chars().enumerate() {
        let (kind, note) = if c == '\u{feff}' {
            ("bom", if i == 0 { "byte order mark at the start".to_string() } else { "zero width no-break space (BOM) inside the text".to_string() })
        } else if is_bidi_control(c) {
            ("bidi", "reorders how the text is displayed".to_string())
        } else if is_invisible(c) {
            ("invisible", "zero-width / invisible".to_string())
        } else if c.is_control() && !matches!(c, '\t' | '\n' | '\r') {
            ("control", "control character".to_string())
        } else if is_unusual_space(c) {
            ("whitespace", if c == '\u{a0}' { "non-breaking space (looks like a space)".to_string() } else { "unusual whitespace (looks like a space)".to_string() })
        } else if let Some(l) = confusable(c) {
            ("confusable", format!("{} — looks like '{l}'", script(c)))
        } else {
            continue;
        };
        n += 1;
        if n <= MAX_FINDINGS {
            out.push(json!({
                "side": side,
                "index": i,
                "char": c.to_string(),
                "codePoint": code_point(c),
                "name": char_name(c),
                "kind": kind,
                "note": note,
            }));
        }
    }
    n
}

fn first_difference(a: &str, b: &str) -> Option<Value> {
    let (av, bv): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let i = av.iter().zip(&bv).position(|(x, y)| x != y).or_else(|| (av.len() != bv.len()).then(|| av.len().min(bv.len())))?;
    let before: String = av[i.saturating_sub(CONTEXT)..i].iter().collect();
    let line = av[..i].iter().filter(|c| **c == '\n').count() + 1;
    let column = i - av[..i].iter().rposition(|c| *c == '\n').map(|p| p + 1).unwrap_or(0) + 1;
    let byte = |v: &[char]| v[..i.min(v.len())].iter().map(|c| c.len_utf8()).sum::<usize>();
    let utf16 = av[..i].iter().map(|c| c.len_utf16()).sum::<usize>();
    Some(json!({
        "index": i,
        "line": line,
        "column": column,
        "utf16Offset": utf16,
        "byteOffsetA": byte(&av),
        "byteOffsetB": byte(&bv),
        "a": av.get(i).map(|c| describe(*c)),
        "b": bv.get(i).map(|c| describe(*c)),
        "before": before,
        "afterA": av[i..(i + CONTEXT).min(av.len())].iter().collect::<String>(),
        "afterB": bv[i..(i + CONTEXT).min(bv.len())].iter().collect::<String>(),
    }))
}

fn char_diff(a: &str, b: &str) -> Option<Vec<Value>> {
    if a.chars().count() > MAX_DIFF_CHARS || b.chars().count() > MAX_DIFF_CHARS {
        return None;
    }
    let mut cfg = TextDiff::configure();
    cfg.algorithm(Algorithm::Myers).timeout(DIFF_TIMEOUT);
    let diff = cfg.diff_chars(a, b);
    let mut segments: Vec<(ChangeTag, String)> = Vec::new();
    for c in diff.iter_all_changes() {
        match segments.last_mut() {
            Some((t, s)) if *t == c.tag() => s.push_str(c.value()),
            _ => segments.push((c.tag(), c.value().to_string())),
        }
    }
    Some(
        segments
            .into_iter()
            .map(|(t, value)| {
                let tag = match t {
                    ChangeTag::Equal => "equal",
                    ChangeTag::Delete => "delete",
                    ChangeTag::Insert => "insert",
                };
                json!({ "tag": tag, "value": value })
            })
            .collect(),
    )
}

/// Plain-language explanations of why two unequal strings differ, most specific first.
fn hints(a: &str, b: &str, eq: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if eq["exact"] == true {
        return out;
    }
    let is = |k: &str| eq[k] == true;
    if is("lineEndings") {
        out.push(format!("Only line endings differ (A: {}, B: {}).", line_endings(a), line_endings(b)));
    } else if is("trimmed") {
        let what = if a.trim_end_matches(['\r', '\n']) == b.trim_end_matches(['\r', '\n']) { "a trailing newline" } else { "leading or trailing whitespace" };
        out.push(format!("Only {what} differs."));
    }
    if is("ignoreCase") && !is("trimmed") {
        out.push("Only letter case differs.".into());
    }
    if is("nfc") {
        out.push("Same text in different Unicode normalization forms (e.g. é as one code point vs e + combining accent).".into());
    } else if is("nfkc") {
        out.push("Equal under compatibility normalization (NFKC): fullwidth forms, ligatures, superscripts or similar.".into());
    }
    if is("invisibleRemoved") {
        out.push("Only invisible characters differ (zero-width spaces, BOM, bidi controls).".into());
    }
    if is("whitespace") && !is("trimmed") && !is("lineEndings") {
        out.push("Only whitespace differs (e.g. non-breaking vs regular space, tabs vs spaces, extra spaces).".into());
    }
    if is("lookalikes") && !is("nfkc") && !is("ignoreCase") {
        out.push("Look-alike characters from other scripts make them different (e.g. Cyrillic 'а' vs Latin 'a').".into());
    }
    if out.is_empty() && is("loose") {
        out.push("Equal once case, whitespace, Unicode form, invisible and look-alike characters are normalized.".into());
    }
    out
}

pub fn compare(a: &str, b: &str) -> Value {
    let eq = json!({
        "exact": a == b,
        "ignoreCase": a.to_lowercase() == b.to_lowercase(),
        "trimmed": a.trim() == b.trim(),
        "lineEndings": unify_newlines(a) == unify_newlines(b),
        "whitespace": collapse_ws(a) == collapse_ws(b),
        "nfc": a.nfc().eq(b.nfc()),
        "nfkc": a.nfkc().eq(b.nfkc()),
        "invisibleRemoved": strip_invisible(a) == strip_invisible(b),
        "lookalikes": skeleton(a) == skeleton(b),
        "loose": loose(a) == loose(b),
    });
    let mut found = Vec::new();
    let na = findings("a", a, &mut found);
    let nb = findings("b", b, &mut found);
    let segments = char_diff(a, b);
    json!({
        "equal": eq,
        "hints": hints(a, b, &eq),
        "a": side_stats(a),
        "b": side_stats(b),
        "firstDifference": first_difference(a, b),
        "segments": segments,
        "diffSkipped": segments.is_none(),
        "findings": found,
        "findingCounts": { "a": na, "b": nb },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings() {
        let r = compare("hello", "hello");
        assert_eq!(r["equal"]["exact"], true);
        assert!(r["firstDifference"].is_null());
        assert_eq!(r["hints"].as_array().unwrap().len(), 0);
        assert_eq!(r["a"]["sha256"], r["b"]["sha256"]);
        assert_eq!(r["a"]["sha256"], "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn case_whitespace_and_line_endings() {
        let r = compare("Hello World", "hello world");
        assert_eq!((r["equal"]["exact"].as_bool(), r["equal"]["ignoreCase"].as_bool()), (Some(false), Some(true)));
        assert!(r["hints"][0].as_str().unwrap().contains("case"));

        let r = compare("a\r\nb\r\n", "a\nb\n");
        assert_eq!(r["equal"]["lineEndings"], true);
        assert_eq!((r["a"]["lineEndings"].as_str(), r["b"]["lineEndings"].as_str()), (Some("CRLF"), Some("LF")));
        assert!(r["hints"][0].as_str().unwrap().contains("CRLF"));
        assert_eq!(r["a"]["whitespace"]["cr"], 2);

        let r = compare("price:\u{a0}10", "price: 10");
        assert_eq!((r["equal"]["whitespace"].as_bool(), r["equal"]["trimmed"].as_bool()), (Some(true), Some(false)));
        let f = &r["findings"][0];
        assert_eq!((f["side"].as_str(), f["codePoint"].as_str(), f["kind"].as_str()), (Some("a"), Some("U+00A0"), Some("whitespace")));
        assert_eq!(f["name"], "NO-BREAK SPACE");

        let r = compare("value\n", "value");
        assert_eq!(r["equal"]["trimmed"], true);
        assert!(r["hints"][0].as_str().unwrap().contains("trailing newline"));
        assert_eq!(r["a"]["trailingNewline"], true);
    }

    #[test]
    fn unicode_normalization_and_lengths() {
        // é precomposed (U+00E9) vs e + U+0301.
        let r = compare("caf\u{e9}", "cafe\u{301}");
        assert_eq!((r["equal"]["exact"].as_bool(), r["equal"]["nfc"].as_bool()), (Some(false), Some(true)));
        assert_eq!((r["a"]["chars"].as_u64(), r["b"]["chars"].as_u64()), (Some(4), Some(5)));
        assert_eq!((r["a"]["graphemes"].as_u64(), r["b"]["graphemes"].as_u64()), (Some(4), Some(4)));
        assert_eq!((r["a"]["utf8Bytes"].as_u64(), r["b"]["utf8Bytes"].as_u64()), (Some(5), Some(6)));
        assert_eq!(r["b"]["isNfc"], false);
        let d = &r["firstDifference"];
        assert_eq!((d["index"].as_u64(), d["a"]["codePoint"].as_str(), d["b"]["codePoint"].as_str()), (Some(3), Some("U+00E9"), Some("U+0065")));

        // Fullwidth → NFKC.
        let r = compare("ＡＢＣ", "ABC");
        assert_eq!((r["equal"]["nfc"].as_bool(), r["equal"]["nfkc"].as_bool()), (Some(false), Some(true)));

        // Emoji: one grapheme, several code points, UTF-16 surrogates.
        let r = compare("👍🏽", "👍");
        assert_eq!((r["a"]["graphemes"].as_u64(), r["a"]["chars"].as_u64(), r["a"]["utf16Units"].as_u64()), (Some(1), Some(2), Some(4)));
    }

    #[test]
    fn invisible_and_lookalike_characters() {
        let r = compare("\u{feff}admin\u{200b}", "admin");
        assert_eq!(r["equal"]["invisibleRemoved"], true);
        assert_eq!(r["a"]["bom"], true);
        let kinds: Vec<&str> = r["findings"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["bom", "invisible"]);
        assert_eq!(r["findings"][1]["codePoint"], "U+200B");

        // Cyrillic а (U+0430) vs Latin a.
        let r = compare("p\u{430}ypal", "paypal");
        assert_eq!((r["equal"]["exact"].as_bool(), r["equal"]["lookalikes"].as_bool()), (Some(false), Some(true)));
        assert_eq!(r["findings"][0]["codePoint"], "U+0430");
        assert!(r["findings"][0]["note"].as_str().unwrap().contains("Cyrillic"));
        assert!(r["hints"].as_array().unwrap().iter().any(|h| h.as_str().unwrap().contains("Look-alike")));
        let d = &r["firstDifference"];
        assert_eq!((d["index"].as_u64(), d["line"].as_u64(), d["column"].as_u64()), (Some(1), Some(1), Some(2)));
    }

    #[test]
    fn first_difference_and_char_diff() {
        let r = compare("line one\nline two", "line one\nline tw0");
        let d = &r["firstDifference"];
        assert_eq!((d["line"].as_u64(), d["column"].as_u64(), d["index"].as_u64()), (Some(2), Some(8), Some(16)));
        assert_eq!(d["before"], "line one\nline tw");
        let segs = r["segments"].as_array().unwrap();
        let tags: Vec<&str> = segs.iter().map(|s| s["tag"].as_str().unwrap()).collect();
        assert_eq!(tags, ["equal", "delete", "insert"]);
        assert_eq!(segs[1]["value"], "o");

        // One string is a prefix of the other.
        let r = compare("abc", "abcd");
        assert_eq!(r["firstDifference"]["index"], 3);
        assert!(r["firstDifference"]["a"].is_null());
        assert_eq!(r["firstDifference"]["b"]["char"], "d");

        // Large inputs skip the character diff but still compare.
        let big = "x".repeat(MAX_DIFF_CHARS + 1);
        let r = compare(&big, "y");
        assert_eq!(r["diffSkipped"], true);
        assert_eq!(r["equal"]["exact"], false);
    }

    #[test]
    fn registered_as_an_op() {
        let out = crate::run("text.compare", "a\u{a0}b", &json!({ "other": "a b" })).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["equal"]["whitespace"], true);
        assert!(crate::list().iter().any(|o| o.id == "text.compare"));
    }
}
