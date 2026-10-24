//! Identifier case conversion. Words split on non-alphanumerics and camel/acronym boundaries:
//! `parseHTTPResponse_v2` → `parse`, `HTTP`, `Response`, `v2`.

pub fn words(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for chunk in input.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = chunk.chars().collect();
        let mut cur = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let prev = i.checked_sub(1).map(|j| chars[j]);
            let next = chars.get(i + 1).copied();
            // `aB` starts a word; so does the last capital of an acronym followed by lowercase (`HTTPResponse`).
            let boundary = prev.is_some_and(|p| {
                (p.is_lowercase() && c.is_uppercase()) || (p.is_uppercase() && c.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
            if boundary && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

pub fn camel(input: &str) -> String {
    words(input)
        .iter()
        .enumerate()
        .map(|(i, w)| if i == 0 { w.to_lowercase() } else { capitalize(w) })
        .collect()
}

pub fn pascal(input: &str) -> String {
    words(input).iter().map(|w| capitalize(w)).collect()
}

pub fn joined(input: &str, sep: &str, upper: bool) -> String {
    words(input).iter().map(|w| if upper { w.to_uppercase() } else { w.to_lowercase() }).collect::<Vec<_>>().join(sep)
}

pub fn title(input: &str) -> String {
    words(input).iter().map(|w| capitalize(w)).collect::<Vec<_>>().join(" ")
}
