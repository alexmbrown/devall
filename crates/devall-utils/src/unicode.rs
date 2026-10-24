//! Unicode string inspector: per-character code points, names and encodings, plus warnings for
//! invisible characters, bidi controls (Trojan Source), unusual whitespace and look-alikes.

use serde_json::{Value, json};

const MAX_CHARS: usize = 2000;

pub(crate) fn control_name(c: char) -> Option<&'static str> {
    Some(match c as u32 {
        0x00 => "NULL",
        0x07 => "BELL",
        0x08 => "BACKSPACE",
        0x09 => "CHARACTER TABULATION",
        0x0A => "LINE FEED",
        0x0B => "LINE TABULATION",
        0x0C => "FORM FEED",
        0x0D => "CARRIAGE RETURN",
        0x1B => "ESCAPE",
        0x7F => "DELETE",
        0x85 => "NEXT LINE",
        _ if c.is_control() => "<control>",
        _ => return None,
    })
}

pub(crate) fn is_invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180E | 0x200B..=0x200F | 0x2060..=0x2064 | 0x206A..=0x206F | 0x3164 | 0xFEFF | 0xFFA0 | 0xE0000..=0xE007F)
}

pub(crate) fn is_bidi_control(c: char) -> bool {
    matches!(c as u32, 0x061C | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069)
}

pub(crate) fn is_unusual_space(c: char) -> bool {
    c.is_whitespace() && !matches!(c, ' ' | '\n' | '\r' | '\t')
}

/// Latin look-alike for common confusables (Cyrillic, Greek, fullwidth, typographic).
pub(crate) fn confusable(c: char) -> Option<char> {
    let v = c as u32;
    if (0xFF01..=0xFF5E).contains(&v) {
        return char::from_u32(v - 0xFEE0);
    }
    Some(match c {
        'а' => 'a',
        'е' => 'e',
        'о' => 'o',
        'р' => 'p',
        'с' => 'c',
        'у' => 'y',
        'х' => 'x',
        'і' => 'i',
        'ј' => 'j',
        'ѕ' => 's',
        'ԁ' => 'd',
        'ԛ' => 'q',
        'ԝ' => 'w',
        'һ' => 'h',
        'А' => 'A',
        'В' => 'B',
        'Е' => 'E',
        'К' => 'K',
        'М' => 'M',
        'Н' => 'H',
        'О' => 'O',
        'Р' => 'P',
        'С' => 'C',
        'Т' => 'T',
        'Х' => 'X',
        'Ү' => 'Y',
        'І' => 'I',
        'Ј' => 'J',
        'Ѕ' => 'S',
        'ο' => 'o',
        'ν' => 'v',
        'ι' => 'i',
        'α' => 'a',
        'Α' => 'A',
        'Β' => 'B',
        'Ε' => 'E',
        'Ζ' => 'Z',
        'Η' => 'H',
        'Ι' => 'I',
        'Κ' => 'K',
        'Μ' => 'M',
        'Ν' => 'N',
        'Ο' => 'O',
        'Ρ' => 'P',
        'Τ' => 'T',
        'Υ' => 'Y',
        'Χ' => 'X',
        'ı' => 'i',
        'ɡ' => 'g',
        'ℓ' => 'l',
        '‐' | '‑' | '‒' | '–' | '−' => '-',
        '‘' | '’' | 'ʼ' | '′' => '\'',
        '“' | '”' | '″' => '"',
        '⁄' | '∕' => '/',
        'ː' | '∶' => ':',
        _ => return None,
    })
}

pub(crate) fn script(c: char) -> &'static str {
    match c as u32 {
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A => "Latin",
        0x370..=0x3FF | 0x1F00..=0x1FFF => "Greek",
        0x400..=0x52F => "Cyrillic",
        0x530..=0x58F => "Armenian",
        0x590..=0x5FF => "Hebrew",
        0x600..=0x6FF | 0x750..=0x77F => "Arabic",
        0x900..=0x97F => "Devanagari",
        0xE00..=0xE7F => "Thai",
        0x1100..=0x11FF | 0xAC00..=0xD7AF => "Hangul",
        0x3040..=0x30FF => "Kana",
        0x4E00..=0x9FFF | 0x3400..=0x4DBF => "Han",
        _ => "Common",
    }
}

fn category(c: char) -> &'static str {
    if c.is_control() {
        "control"
    } else if ('\u{300}'..='\u{36F}').contains(&c) || ('\u{FE20}'..='\u{FE2F}').contains(&c) || ('\u{1AB0}'..='\u{1AFF}').contains(&c) {
        "combining mark"
    } else if c.is_alphabetic() {
        "letter"
    } else if c.is_numeric() {
        "number"
    } else if c.is_whitespace() {
        "whitespace"
    } else if c.is_ascii_punctuation() || matches!(c as u32, 0x2010..=0x2027 | 0x3000..=0x303F) {
        "punctuation"
    } else if matches!(c as u32, 0xFE00..=0xFE0F | 0xE0100..=0xE01EF) {
        "variation selector"
    } else if matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF) {
        "emoji / symbol"
    } else {
        "symbol / other"
    }
}

pub fn inspect(input: &str) -> Value {
    let mut chars = Vec::new();
    let mut invisible = Vec::new();
    let mut bidi = Vec::new();
    let mut spaces = Vec::new();
    let mut confusables = Vec::new();
    let mut utf16_index = 0usize;
    for (i, (byte, c)) in input.char_indices().enumerate() {
        let mut flags: Vec<String> = Vec::new();
        let cp = format!("U+{:04X}", c as u32);
        if is_bidi_control(c) {
            flags.push("bidi control".into());
            bidi.push(cp.clone());
        } else if is_invisible(c) {
            flags.push("invisible".into());
            invisible.push(cp.clone());
        }
        if is_unusual_space(c) {
            flags.push("unusual whitespace".into());
            spaces.push(cp.clone());
        }
        if let Some(l) = confusable(c) {
            flags.push(format!("looks like '{l}'"));
            confusables.push(format!("{c} ({cp}) ≈ {l}"));
        }
        if i < MAX_CHARS {
            let mut utf16 = [0u16; 2];
            let mut utf8 = [0u8; 4];
            let name = control_name(c).map(str::to_string).or_else(|| unicode_names2::name(c).map(|n| n.to_string()));
            chars.push(json!({
                "index": i,
                "byteOffset": byte,
                "utf16Offset": utf16_index,
                "char": if c.is_control() || is_invisible(c) || is_bidi_control(c) { String::new() } else { c.to_string() },
                "codePoint": cp,
                "decimal": c as u32,
                "name": name,
                "category": category(c),
                "script": script(c),
                "utf8": c.encode_utf8(&mut utf8).bytes().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
                "utf16": c.encode_utf16(&mut utf16).iter().map(|u| format!("{u:04X}")).collect::<Vec<_>>().join(" "),
                "flags": flags,
            }));
        }
        utf16_index += c.len_utf16();
    }
    let mut warnings: Vec<String> = Vec::new();
    if !bidi.is_empty() {
        warnings.push(format!("{} bidirectional control character(s) ({}) can reorder how text displays (\"Trojan Source\")", bidi.len(), dedup(&bidi)));
    }
    if !invisible.is_empty() {
        warnings.push(format!("{} invisible character(s): {}", invisible.len(), dedup(&invisible)));
    }
    if !spaces.is_empty() {
        warnings.push(format!("{} non-standard whitespace character(s): {}", spaces.len(), dedup(&spaces)));
    }
    if !confusables.is_empty() {
        let mut list = confusables.clone();
        list.dedup();
        list.truncate(10);
        warnings.push(format!("Look-alike characters: {}", list.join(", ")));
    }
    // Mixed scripts inside a single word (classic homograph trick).
    for word in input.split(|c: char| !c.is_alphanumeric()) {
        let mut scripts: Vec<&str> = word.chars().map(script).filter(|s| *s != "Common").collect();
        scripts.sort_unstable();
        scripts.dedup();
        if scripts.len() > 1 {
            warnings.push(format!("Mixed scripts in \"{word}\": {}", scripts.join(" + ")));
        }
    }
    let count = input.chars().count();
    let combining = input.chars().filter(|c| category(*c) == "combining mark").count();
    json!({
        "counts": {
            "codePoints": count,
            "utf16Units": utf16_index,
            "utf8Bytes": input.len(),
            "approxGraphemes": count - combining,
            "lines": if input.is_empty() { 0 } else { input.lines().count() },
            "nonAscii": input.chars().filter(|c| !c.is_ascii()).count(),
        },
        "warnings": warnings,
        "truncated": count > MAX_CHARS,
        "characters": chars,
    })
}

fn dedup(v: &[String]) -> String {
    let mut u: Vec<&String> = v.iter().collect();
    u.sort();
    u.dedup();
    u.iter().take(8).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}
