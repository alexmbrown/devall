//! Recognizing identifier-like path segments, shared by stub generation (regex patterns) and
//! OpenAPI inference (`{param}` templates).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SegKind {
    Literal,
    Int,
    Uuid,
    Date,
    Ulid,
    Hex,
    /// Opaque ids such as `cus_8f3K2j1x` (≥ 8 chars, letters and ≥ 2 digits).
    Token,
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.as_bytes()[0].is_ascii_digit()
        && s.as_bytes()[0] <= b'7'
        && s.chars().all(|c| c.is_ascii_digit() || (c.is_ascii_alphabetic() && !matches!(c.to_ascii_uppercase(), 'I' | 'L' | 'O' | 'U')))
        && s.chars().any(|c| c.is_ascii_alphabetic())
}

pub(crate) fn classify(seg: &str) -> SegKind {
    if seg.is_empty() {
        return SegKind::Literal;
    }
    if seg.bytes().all(|c| c.is_ascii_digit()) {
        return SegKind::Int;
    }
    if is_uuid(seg) {
        return SegKind::Uuid;
    }
    if is_date(seg) {
        return SegKind::Date;
    }
    if is_ulid(seg) {
        return SegKind::Ulid;
    }
    let digits = seg.bytes().filter(u8::is_ascii_digit).count();
    if seg.len() >= 16 && seg.bytes().all(|c| c.is_ascii_hexdigit()) && digits > 0 {
        return SegKind::Hex;
    }
    let token_chars = seg.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~'));
    if seg.len() >= 8 && token_chars && digits >= 2 && seg.bytes().any(|c| c.is_ascii_alphabetic()) {
        return SegKind::Token;
    }
    SegKind::Literal
}

/// Regex matching a whole segment of this kind.
pub(crate) fn regex_for(kind: SegKind) -> &'static str {
    match kind {
        SegKind::Literal => "",
        SegKind::Int => "[0-9]+",
        SegKind::Uuid => "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        SegKind::Date => "[0-9]{4}-[0-9]{2}-[0-9]{2}",
        SegKind::Ulid => "[0-9A-Za-z]{26}",
        SegKind::Hex => "[0-9a-fA-F]+",
        SegKind::Token => "[^/]+",
    }
}

/// A full-path regex with id-like segments generalized, or `None` when nothing was.
pub(crate) fn path_pattern(path: &str) -> Option<String> {
    let mut any = false;
    let parts: Vec<String> = path
        .split('/')
        .map(|seg| match classify(seg) {
            SegKind::Literal => regex::escape(seg),
            k => {
                any = true;
                regex_for(k).to_string()
            }
        })
        .collect();
    any.then(|| parts.join("/"))
}

/// `users` → `user`, `categories` → `category`, `boxes` → `box`.
pub(crate) fn singular(word: &str) -> String {
    let w = word;
    if let Some(stem) = w.strip_suffix("ies").filter(|s| !s.is_empty()) {
        return format!("{stem}y");
    }
    for suf in ["sses", "xes", "ches", "shes", "zes"] {
        if w.ends_with(suf) {
            return w[..w.len() - 2].to_string();
        }
    }
    if w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") && w.len() > 1 {
        return w[..w.len() - 1].to_string();
    }
    w.to_string()
}

/// `order-items` → `orderItems` (ASCII letters/digits only; never empty).
pub(crate) fn camel(word: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in word.chars() {
        if c.is_ascii_alphanumeric() {
            if out.is_empty() {
                if c.is_ascii_digit() {
                    out.push('p');
                }
                out.push(c.to_ascii_lowercase());
            } else if upper {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push(c);
            }
            upper = false;
        } else {
            upper = !out.is_empty();
        }
    }
    if out.is_empty() { "param".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_segments() {
        assert_eq!(classify("42"), SegKind::Int);
        assert_eq!(classify("3fa85f64-5717-4562-b3fc-2c963f66afa6"), SegKind::Uuid);
        assert_eq!(classify("2024-01-31"), SegKind::Date);
        assert_eq!(classify("01HZX3K9M2ZC4QW7Y8V6T5R4S3"), SegKind::Ulid);
        assert_eq!(classify("507f1f77bcf86cd799439011"), SegKind::Hex);
        assert_eq!(classify("cus_8f3K2j1x"), SegKind::Token);
        for lit in ["users", "v1", "oauth2", "base64", "me", "api", "deadbeef", "order-items"] {
            assert_eq!(classify(lit), SegKind::Literal, "{lit}");
        }
    }

    #[test]
    fn patterns() {
        assert_eq!(
            path_pattern("/users/42/orders/3fa85f64-5717-4562-b3fc-2c963f66afa6").unwrap(),
            "/users/[0-9]+/orders/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
        );
        assert_eq!(path_pattern("/a.b/7").unwrap(), "/a\\.b/[0-9]+");
        assert!(path_pattern("/users/me").is_none());
    }

    #[test]
    fn words() {
        assert_eq!(singular("users"), "user");
        assert_eq!(singular("categories"), "category");
        assert_eq!(singular("boxes"), "box");
        assert_eq!(singular("status"), "status");
        assert_eq!(singular("address"), "address");
        assert_eq!(camel("order-items"), "orderItems");
        assert_eq!(camel("2fa"), "p2fa");
        assert_eq!(camel("--"), "param");
    }
}
