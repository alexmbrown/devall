//! Colour conversion (HEX / RGB / HSL / HSV / OKLab / OKLCH / CSS names) and WCAG 2.x contrast.

use serde_json::{Value, json};

use crate::{Result, invalid};

/// sRGB in 0..=1 plus alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4), ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4),
    ("black", 0x000000), ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a), ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e), ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c), ("cyan", 0x00ffff),
    ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b), ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc), ("darkred", 0x8b0000), ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b), ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22), ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700), ("goldenrod", 0xdaa520), ("gray", 0x808080),
    ("green", 0x008000), ("greenyellow", 0xadff2f), ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c), ("indigo", 0x4b0082),
    ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3), ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a), ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32), ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa), ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371), ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5), ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23), ("orange", 0xffa500),
    ("orangered", 0xff4500), ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb), ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080),
    ("rebeccapurple", 0x663399), ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072), ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd), ("slategray", 0x708090),
    ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3), ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

fn from_u32(v: u32) -> Rgba {
    Rgba { r: ((v >> 16) & 0xff) as f64 / 255.0, g: ((v >> 8) & 0xff) as f64 / 255.0, b: (v & 0xff) as f64 / 255.0, a: 1.0 }
}

/// Numbers inside `fn(...)`: commas, spaces and `/` all separate.
fn components(body: &str) -> Vec<&str> {
    body.split([',', ' ', '/']).map(str::trim).filter(|s| !s.is_empty()).collect()
}

fn num(s: &str, percent_scale: f64) -> Result<f64> {
    let bad = || invalid(format!("bad number {s:?}"));
    if let Some(p) = s.strip_suffix('%') {
        return p.parse::<f64>().map(|v| v / 100.0 * percent_scale).map_err(|_| bad());
    }
    s.trim_end_matches("deg").parse::<f64>().map_err(|_| bad())
}

fn alpha(parts: &[&str], idx: usize) -> Result<f64> {
    parts.get(idx).map_or(Ok(1.0), |a| num(a, 1.0)).map(|a| a.clamp(0.0, 1.0))
}

fn hue_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r + m, g + m, b + m)
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(c: f64) -> f64 {
    if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn to_oklab(c: Rgba) -> (f64, f64, f64) {
    let (r, g, b) = (to_linear(c.r), to_linear(c.g), to_linear(c.b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    (
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    )
}

/// OKLab → sRGB; the bool reports whether the colour was outside the sRGB gamut (and clamped).
fn from_oklab(l: f64, a: f64, b: f64, alpha: f64) -> (Rgba, bool) {
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    let r = from_linear(4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_);
    let g = from_linear(-1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_);
    let bl = from_linear(-0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_);
    let out = [r, g, bl].iter().any(|c| !(-0.001..=1.001).contains(c));
    (Rgba { r: r.clamp(0.0, 1.0), g: g.clamp(0.0, 1.0), b: bl.clamp(0.0, 1.0), a: alpha }, out)
}

fn parse(input: &str) -> Result<(Rgba, bool)> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(invalid("enter a colour: #3b82f6, rgb(59 130 246), hsl(217 91% 60%), oklch(0.62 0.19 260) or a CSS name"));
    }
    if s == "transparent" {
        return Ok((Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, false));
    }
    if let Some(&(_, v)) = NAMED.iter().find(|(n, _)| *n == s) {
        return Ok((from_u32(v), false));
    }
    let hex = s.strip_prefix('#').unwrap_or(&s);
    if hex.chars().all(|c| c.is_ascii_hexdigit()) && matches!(hex.len(), 3 | 4 | 6 | 8) {
        let full: String = if hex.len() <= 4 { hex.chars().flat_map(|c| [c, c]).collect() } else { hex.to_string() };
        let v = u32::from_str_radix(&full, 16).map_err(|e| invalid(e.to_string()))?;
        return Ok(if full.len() == 8 {
            let mut c = from_u32(v >> 8);
            c.a = (v & 0xff) as f64 / 255.0;
            (c, false)
        } else {
            (from_u32(v), false)
        });
    }
    let (func, body) = s.split_once('(').ok_or_else(|| invalid(format!("unrecognized colour {input:?}")))?;
    let body = body.trim_end_matches(')');
    let p = components(body);
    if p.len() < 3 {
        return Err(invalid("expected three components"));
    }
    match func.trim() {
        "rgb" | "rgba" => Ok((
            Rgba {
                r: num(p[0], 255.0)?.clamp(0.0, 255.0) / 255.0,
                g: num(p[1], 255.0)?.clamp(0.0, 255.0) / 255.0,
                b: num(p[2], 255.0)?.clamp(0.0, 255.0) / 255.0,
                a: alpha(&p, 3)?,
            },
            false,
        )),
        "hsl" | "hsla" => {
            let (r, g, b) = hue_to_rgb(num(p[0], 360.0)?, num(p[1], 100.0)?.clamp(0.0, 100.0) / 100.0, num(p[2], 100.0)?.clamp(0.0, 100.0) / 100.0);
            Ok((Rgba { r, g, b, a: alpha(&p, 3)? }, false))
        }
        "hsv" | "hsb" => {
            let (h, sv, v) = (num(p[0], 360.0)?, num(p[1], 100.0)?.clamp(0.0, 100.0) / 100.0, num(p[2], 100.0)?.clamp(0.0, 100.0) / 100.0);
            let l = v * (1.0 - sv / 2.0);
            let sl = if l == 0.0 || l == 1.0 { 0.0 } else { (v - l) / l.min(1.0 - l) };
            let (r, g, b) = hue_to_rgb(h, sl, l);
            Ok((Rgba { r, g, b, a: alpha(&p, 3)? }, false))
        }
        "oklch" => {
            let l = num(p[0], 1.0)?;
            let c = num(p[1], 0.4)?;
            let h = num(p[2], 360.0)?.to_radians();
            Ok(from_oklab(l, c * h.cos(), c * h.sin(), alpha(&p, 3)?))
        }
        "oklab" => Ok(from_oklab(num(p[0], 1.0)?, num(p[1], 0.4)?, num(p[2], 0.4)?, alpha(&p, 3)?)),
        other => Err(invalid(format!("unsupported colour function {other}()"))),
    }
}

fn byte(c: f64) -> u8 {
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

fn hsl(c: Rgba) -> (f64, f64, f64) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    (hue(c, max, d), s, l)
}

fn hue(c: Rgba, max: f64, d: f64) -> f64 {
    let h = if max == c.r {
        ((c.g - c.b) / d).rem_euclid(6.0)
    } else if max == c.g {
        (c.b - c.r) / d + 2.0
    } else {
        (c.r - c.g) / d + 4.0
    };
    h * 60.0
}

fn hsv(c: Rgba) -> (f64, f64, f64) {
    let max = c.r.max(c.g).max(c.b);
    let d = max - c.r.min(c.g).min(c.b);
    let h = if d == 0.0 { 0.0 } else { hue(c, max, d) };
    (h, if max == 0.0 { 0.0 } else { d / max }, max)
}

fn luminance(c: Rgba) -> f64 {
    0.2126 * to_linear(c.r) + 0.7152 * to_linear(c.g) + 0.0722 * to_linear(c.b)
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn hex_of(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

pub fn convert(input: &str, against: &str) -> Result<Value> {
    let (c, out_of_gamut) = parse(input)?;
    let (h, s, l) = hsl(c);
    let (hv, sv, vv) = hsv(c);
    let (ol, oa, ob) = to_oklab(c);
    let chroma = (oa * oa + ob * ob).sqrt();
    let oh = if chroma < 1e-4 { 0.0 } else { ob.atan2(oa).to_degrees().rem_euclid(360.0) };
    let alpha_suffix = if c.a < 1.0 { format!(" / {}", r3(c.a)) } else { String::new() };
    let nearest = NAMED
        .iter()
        .map(|(n, v)| {
            let (l2, a2, b2) = to_oklab(from_u32(*v));
            (n, (ol - l2).powi(2) + (oa - a2).powi(2) + (ob - b2).powi(2))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, d)| json!({ "name": n, "exact": d < 1e-9 }));
    let lum = luminance(c);
    let mut out = json!({
        "hex": hex_of(c),
        "hexAlpha": if c.a < 1.0 { json!(format!("{}{:02x}", hex_of(c), byte(c.a))) } else { Value::Null },
        "rgb": format!("rgb({} {} {}{alpha_suffix})", byte(c.r), byte(c.g), byte(c.b)),
        "hsl": format!("hsl({} {}% {}%{alpha_suffix})", r1(h), r1(s * 100.0), r1(l * 100.0)),
        "hsv": format!("hsv({} {}% {}%)", r1(hv), r1(sv * 100.0), r1(vv * 100.0)),
        "oklab": format!("oklab({} {} {}{alpha_suffix})", r3(ol), r3(oa), r3(ob)),
        "oklch": format!("oklch({} {} {}{alpha_suffix})", r3(ol), r3(chroma), r1(oh)),
        "values": {
            "r": byte(c.r), "g": byte(c.g), "b": byte(c.b), "alpha": r3(c.a),
            "h": r1(h), "s": r1(s * 100.0), "l": r1(l * 100.0),
        },
        "nearestNamed": nearest,
        "relativeLuminance": r3(lum),
        "bestTextColor": if (lum + 0.05) / 0.05 >= 1.05 / (lum + 0.05) { "black" } else { "white" },
        "outOfSrgbGamut": out_of_gamut,
    });
    if !against.is_empty() {
        let (bg, _) = parse(against)?;
        let l2 = luminance(bg);
        let ratio = (lum.max(l2) + 0.05) / (lum.min(l2) + 0.05);
        let ratio = (ratio * 100.0).round() / 100.0;
        out["contrast"] = json!({
            "against": hex_of(bg),
            "ratio": ratio,
            "aaNormalText": ratio >= 4.5,
            "aaLargeText": ratio >= 3.0,
            "aaaNormalText": ratio >= 7.0,
            "aaaLargeText": ratio >= 4.5,
            "uiComponents": ratio >= 3.0,
        });
    }
    Ok(out)
}
