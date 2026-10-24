//! Locale-aware primitives from the `fake` crate behind a runtime [`Locale`] switch, plus the few
//! generators `fake` doesn't have (product names, safe e-mail domains, ASCII folding).

use fake::Fake;
use fake::faker::impls::address::CityNameGenFn;
use fake::faker::address::raw::{BuildingNumber, CityName, CountryCode, CountryName, StateName, StreetName, ZipCode};
use fake::faker::company::raw::{CatchPhrase, CompanyName};
use fake::faker::currency::raw::CurrencyCode;
use fake::faker::internet::raw::{DomainSuffix, IPv4, IPv6, Password, UserAgent};
use fake::faker::job::raw::Title as JobTitle;
use fake::faker::lorem::raw::{Sentence, Word};
use fake::faker::name::raw::{FirstName, LastName};
use fake::faker::phone_number::raw::PhoneNumber;
use fake::locales::{AR_SA, CY_GB, DE_DE, Data, EN, FA_IR, FR_FR, IT_IT, JA_JP, NL_NL, PT_BR, PT_PT, TR_TR, ZH_CN, ZH_TW};
use rand::RngExt;

/// The deterministic, portable RNG every generator uses (one stream per field).
pub type DRng = rand::rngs::Xoshiro256PlusPlus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    FrFr,
    DeDe,
    ItIt,
    PtBr,
    PtPt,
    NlNl,
    JaJp,
    ZhCn,
    ZhTw,
    ArSa,
    TrTr,
    FaIr,
    CyGb,
}

/// `(code, label)` of every supported locale, in display order.
pub const LOCALES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr_fr", "French"),
    ("de_de", "German"),
    ("it_it", "Italian"),
    ("pt_br", "Portuguese (Brazil)"),
    ("pt_pt", "Portuguese (Portugal)"),
    ("nl_nl", "Dutch"),
    ("ja_jp", "Japanese"),
    ("zh_cn", "Chinese (Simplified)"),
    ("zh_tw", "Chinese (Traditional)"),
    ("ar_sa", "Arabic"),
    ("tr_tr", "Turkish"),
    ("fa_ir", "Persian"),
    ("cy_gb", "Welsh"),
];

impl Locale {
    /// Accepts the codes in [`LOCALES`] plus short / BCP-47 forms (`de`, `pt-BR`, `en_US`).
    pub fn parse(code: &str) -> Option<Locale> {
        let c = code.trim().to_ascii_lowercase().replace('-', "_");
        Some(match c.as_str() {
            "" | "en" | "en_us" | "en_gb" | "en_au" | "en_ca" => Locale::En,
            "fr" | "fr_fr" | "fr_ca" | "fr_be" => Locale::FrFr,
            "de" | "de_de" | "de_at" | "de_ch" => Locale::DeDe,
            "it" | "it_it" => Locale::ItIt,
            "pt_br" | "br" => Locale::PtBr,
            "pt" | "pt_pt" => Locale::PtPt,
            "nl" | "nl_nl" | "nl_be" => Locale::NlNl,
            "ja" | "ja_jp" | "jp" => Locale::JaJp,
            "zh" | "zh_cn" | "zh_hans" => Locale::ZhCn,
            "zh_tw" | "zh_hant" | "zh_hk" => Locale::ZhTw,
            "ar" | "ar_sa" => Locale::ArSa,
            "tr" | "tr_tr" => Locale::TrTr,
            "fa" | "fa_ir" => Locale::FaIr,
            "cy" | "cy_gb" => Locale::CyGb,
            _ => return None,
        })
    }

    pub fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::FrFr => "fr_fr",
            Locale::DeDe => "de_de",
            Locale::ItIt => "it_it",
            Locale::PtBr => "pt_br",
            Locale::PtPt => "pt_pt",
            Locale::NlNl => "nl_nl",
            Locale::JaJp => "ja_jp",
            Locale::ZhCn => "zh_cn",
            Locale::ZhTw => "zh_tw",
            Locale::ArSa => "ar_sa",
            Locale::TrTr => "tr_tr",
            Locale::FaIr => "fa_ir",
            Locale::CyGb => "cy_gb",
        }
    }

    /// Family name first, no space (zh/ja).
    pub(crate) fn family_first(self) -> bool {
        matches!(self, Locale::JaJp | Locale::ZhCn | Locale::ZhTw)
    }
}

/// A locale-dependent text primitive.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Text {
    FirstName,
    LastName,
    Phone,
    StreetName,
    BuildingNumber,
    City,
    State,
    Zip,
    Country,
    CountryCode,
    Company,
    JobTitle,
    CatchPhrase,
    Currency,
    Word,
    DomainSuffix,
    UserAgent,
    Ipv4,
    Ipv6,
    /// Words in `min..=max`, capitalized, with a full stop.
    Sentence(u32, u32),
    /// Characters in `min..=max`.
    Password(u32, u32),
}

pub(crate) fn text(locale: Locale, t: Text, rng: &mut DRng) -> String {
    match locale {
        Locale::En => text_in(EN, t, rng),
        Locale::FrFr => text_in(FR_FR, t, rng),
        Locale::DeDe => text_in(DE_DE, t, rng),
        Locale::ItIt => text_in(IT_IT, t, rng),
        Locale::PtBr => text_in(PT_BR, t, rng),
        Locale::PtPt => text_in(PT_PT, t, rng),
        Locale::NlNl => text_in(NL_NL, t, rng),
        Locale::JaJp => text_in(JA_JP, t, rng),
        Locale::ZhCn => text_in(ZH_CN, t, rng),
        Locale::ZhTw => text_in(ZH_TW, t, rng),
        Locale::ArSa => text_in(AR_SA, t, rng),
        Locale::TrTr => text_in(TR_TR, t, rng),
        Locale::FaIr => text_in(FA_IR, t, rng),
        Locale::CyGb => text_in(CY_GB, t, rng),
    }
}

fn text_in<L: Data + Copy + CityNameGenFn>(l: L, t: Text, rng: &mut DRng) -> String {
    match t {
        Text::FirstName => FirstName(l).fake_with_rng(rng),
        Text::LastName => LastName(l).fake_with_rng(rng),
        Text::Phone => PhoneNumber(l).fake_with_rng(rng),
        Text::StreetName => StreetName(l).fake_with_rng(rng),
        Text::BuildingNumber => BuildingNumber(l).fake_with_rng(rng),
        Text::City => CityName(l).fake_with_rng(rng),
        Text::State => StateName(l).fake_with_rng(rng),
        Text::Zip => ZipCode(l).fake_with_rng(rng),
        Text::Country => CountryName(l).fake_with_rng(rng),
        Text::CountryCode => CountryCode(l).fake_with_rng(rng),
        Text::Company => CompanyName(l).fake_with_rng(rng),
        Text::JobTitle => JobTitle(l).fake_with_rng(rng),
        Text::CatchPhrase => CatchPhrase(l).fake_with_rng(rng),
        Text::Currency => CurrencyCode(l).fake_with_rng(rng),
        Text::Word => Word(l).fake_with_rng(rng),
        Text::DomainSuffix => DomainSuffix(l).fake_with_rng(rng),
        Text::UserAgent => UserAgent(l).fake_with_rng(rng),
        Text::Ipv4 => IPv4(l).fake_with_rng(rng),
        Text::Ipv6 => IPv6(l).fake_with_rng(rng),
        Text::Sentence(min, max) => {
            let (min, max) = ordered(min.max(1), max.max(1));
            let s: String = Sentence(l, min as usize..max as usize + 1).fake_with_rng(rng);
            capitalize(&s)
        }
        Text::Password(min, max) => {
            let (min, max) = ordered(min.max(1), max.max(1));
            Password(l, min as usize..max as usize + 1).fake_with_rng(rng)
        }
    }
}

fn ordered(a: u32, b: u32) -> (u32, u32) {
    if a <= b { (a, b) } else { (b, a) }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub(crate) fn paragraph(locale: Locale, sentences: u32, rng: &mut DRng) -> String {
    let n = sentences.max(1);
    let mut out = String::new();
    for i in 0..n {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&text(locale, Text::Sentence(4, 12), rng));
    }
    out
}

pub(crate) fn lorem_words(locale: Locale, words: u32, rng: &mut DRng) -> String {
    let mut out = String::new();
    for i in 0..words.max(1) {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&text(locale, Text::Word, rng));
    }
    out
}

/// Lowercase ASCII letters and digits only (`Zoë O'Brien` → `zoeobrien`).
pub(crate) fn ascii_slug(s: &str) -> String {
    deunicode::deunicode(s).chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

const ADJECTIVES: &[&str] = &[
    "Ergonomic", "Rustic", "Sleek", "Handcrafted", "Practical", "Intelligent", "Gorgeous", "Incredible", "Fantastic",
    "Licensed", "Refined", "Generic", "Small", "Awesome", "Unbranded", "Electronic", "Modern", "Recycled", "Bespoke",
    "Luxurious", "Oriental", "Tasty", "Handmade", "Elegant", "Durable", "Compact",
];
const MATERIALS: &[&str] = &[
    "Steel", "Wooden", "Concrete", "Plastic", "Cotton", "Granite", "Rubber", "Metal", "Soft", "Fresh", "Frozen",
    "Bronze", "Silk", "Wool", "Leather", "Ceramic", "Bamboo", "Glass", "Marble", "Linen",
];
const PRODUCTS: &[&str] = &[
    "Chair", "Car", "Computer", "Keyboard", "Mouse", "Bike", "Ball", "Gloves", "Pants", "Shirt", "Table", "Shoes",
    "Hat", "Towels", "Soap", "Tuna", "Chicken", "Fish", "Cheese", "Bacon", "Pizza", "Salad", "Sausages", "Chips",
    "Lamp", "Backpack", "Watch", "Mug", "Headphones", "Wallet", "Desk", "Sofa", "Blender", "Kettle", "Jacket",
];

pub(crate) fn product_name(rng: &mut DRng) -> String {
    format!("{} {} {}", pick(ADJECTIVES, rng), pick(MATERIALS, rng), pick(PRODUCTS, rng))
}

/// Reserved documentation domains (RFC 2606): generated addresses can never reach a real inbox.
pub(crate) const SAFE_EMAIL_DOMAINS: &[&str] = &["example.com", "example.net", "example.org"];

pub(crate) fn pick<'a>(items: &[&'a str], rng: &mut DRng) -> &'a str {
    items[rng.random_range(0..items.len())]
}

pub(crate) fn round_to(x: f64, decimals: u32) -> f64 {
    let d = decimals.min(12) as i32;
    let p = 10f64.powi(d);
    let r = (x * p).round() / p;
    if r == 0.0 { 0.0 } else { r }
}
