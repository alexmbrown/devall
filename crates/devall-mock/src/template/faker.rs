//! `{{random 'Name.firstName'}}`: Faker-style keys (as used by WireMock's faker extension /
//! Datafaker) backed by the `fake` crate. Keys are matched case-insensitively and `first_name`
//! equals `firstName`.

use fake::Fake;
use fake::faker::address::en::{
    BuildingNumber, CityName, CountryCode, CountryName, Latitude, Longitude, SecondaryAddress, StateAbbr, StateName, StreetName, StreetSuffix,
    TimeZone, ZipCode,
};
use fake::faker::barcode::en::{Isbn10, Isbn13};
use fake::faker::boolean::en::Boolean;
use fake::faker::color::en::{Color, HexColor, RgbColor};
use fake::faker::company::en::{Bs, Buzzword, CatchPhrase, CompanyName, CompanySuffix, Industry, Profession};
use fake::faker::creditcard::en::CreditCardNumber;
use fake::faker::currency::en::{CurrencyCode, CurrencyName, CurrencySymbol};
use fake::faker::filesystem::en::{FileExtension, FileName, FilePath, MimeType, Semver};
use fake::faker::finance::en::{Bic, Isin};
use fake::faker::internet::en::{DomainSuffix, FreeEmail, IPv4, IPv6, MACAddress, Password, SafeEmail, UserAgent, Username};
use fake::faker::job::en::{Field, Position, Seniority, Title as JobTitle};
use fake::faker::lorem::en::{Paragraph, Sentence, Word, Words};
use fake::faker::name::en::{FirstName, LastName, Name, Suffix, Title};
use fake::faker::number::en::Digit;
use fake::faker::phone_number::en::{CellNumber, PhoneNumber};

fn word() -> String {
    Word().fake()
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn pick(items: &[&str]) -> String {
    items[fastrand::usize(..items.len())].to_string()
}

fn domain() -> String {
    format!("{}.{}", word(), DomainSuffix().fake::<String>())
}

/// The keys `{{random '…'}}` understands (matched case-insensitively; `_`, `-` and spaces ignored).
pub const KEYS: &[&str] = &[
    "Name.firstName",
    "Name.lastName",
    "Name.fullName",
    "Name.name",
    "Name.nameWithMiddle",
    "Name.prefix",
    "Name.title",
    "Name.suffix",
    "Name.username",
    "Internet.emailAddress",
    "Internet.safeEmailAddress",
    "Internet.domainName",
    "Internet.domainSuffix",
    "Internet.url",
    "Internet.ipV4Address",
    "Internet.ipV6Address",
    "Internet.macAddress",
    "Internet.password",
    "Internet.userAgent",
    "Internet.uuid",
    "Internet.slug",
    "Address.city",
    "Address.streetAddress",
    "Address.streetName",
    "Address.buildingNumber",
    "Address.secondaryAddress",
    "Address.zipCode",
    "Address.postcode",
    "Address.country",
    "Address.countryCode",
    "Address.state",
    "Address.stateAbbr",
    "Address.latitude",
    "Address.longitude",
    "Address.timeZone",
    "Address.fullAddress",
    "PhoneNumber.phoneNumber",
    "PhoneNumber.cellPhone",
    "Company.name",
    "Company.industry",
    "Company.buzzword",
    "Company.catchPhrase",
    "Company.bs",
    "Company.profession",
    "Company.suffix",
    "Company.url",
    "Lorem.word",
    "Lorem.words",
    "Lorem.sentence",
    "Lorem.paragraph",
    "Lorem.characters",
    "Job.title",
    "Job.position",
    "Job.field",
    "Job.seniority",
    "Number.digit",
    "Number.randomDigit",
    "Number.randomNumber",
    "Bool.bool",
    "Color.name",
    "Color.hex",
    "Color.rgb",
    "Finance.creditCard",
    "Finance.bic",
    "Finance.isin",
    "Currency.code",
    "Currency.name",
    "Currency.symbol",
    "Code.isbn10",
    "Code.isbn13",
    "File.fileName",
    "File.extension",
    "File.filePath",
    "File.mimeType",
    "App.version",
    "App.name",
    "Commerce.productName",
    "Commerce.price",
    "Commerce.department",
    "Commerce.color",
    "Date.birthday",
    "Date.past",
    "Date.future",
    "IdNumber.valid",
];

pub fn fake_value(key: &str) -> Option<String> {
    let k: String = key.trim().chars().filter(|c| !matches!(c, '_' | ' ' | '-')).collect::<String>().to_ascii_lowercase();
    Some(match k.as_str() {
        "name.firstname" => FirstName().fake(),
        "name.lastname" => LastName().fake(),
        "name.fullname" | "name.name" => Name().fake(),
        "name.namewithmiddle" => format!("{} {} {}", FirstName().fake::<String>(), FirstName().fake::<String>(), LastName().fake::<String>()),
        "name.prefix" | "name.title" => Title().fake(),
        "name.suffix" => Suffix().fake(),
        "name.username" | "internet.username" => Username().fake(),
        "internet.emailaddress" | "internet.email" => FreeEmail().fake(),
        "internet.safeemailaddress" => SafeEmail().fake(),
        "internet.domainname" => domain(),
        "internet.domainsuffix" => DomainSuffix().fake(),
        "internet.url" | "company.url" => format!("https://www.{}", domain()),
        "internet.ipv4address" | "internet.ipv4" | "internet.ip" => IPv4().fake(),
        "internet.ipv6address" | "internet.ipv6" => IPv6().fake(),
        "internet.macaddress" => MACAddress().fake(),
        "internet.password" => Password(10..16).fake(),
        "internet.useragent" | "internet.useragentany" => UserAgent().fake(),
        "internet.uuid" | "uuid" => uuid::Uuid::new_v4().to_string(),
        "internet.slug" => format!("{}-{}", word(), word()),
        "address.city" | "address.cityname" => CityName().fake(),
        "address.streetaddress" => format!("{} {} {}", BuildingNumber().fake::<String>(), StreetName().fake::<String>(), StreetSuffix().fake::<String>()),
        "address.streetname" => StreetName().fake(),
        "address.buildingnumber" => BuildingNumber().fake(),
        "address.secondaryaddress" => SecondaryAddress().fake(),
        "address.zipcode" | "address.postcode" => ZipCode().fake(),
        "address.country" => CountryName().fake(),
        "address.countrycode" => CountryCode().fake(),
        "address.state" => StateName().fake(),
        "address.stateabbr" => StateAbbr().fake(),
        "address.latitude" => Latitude().fake(),
        "address.longitude" => Longitude().fake(),
        "address.timezone" => TimeZone().fake(),
        "address.fulladdress" => format!(
            "{} {} {}, {}, {} {}",
            BuildingNumber().fake::<String>(),
            StreetName().fake::<String>(),
            StreetSuffix().fake::<String>(),
            CityName().fake::<String>(),
            StateAbbr().fake::<String>(),
            ZipCode().fake::<String>()
        ),
        "phonenumber.phonenumber" | "phonenumber.phone" => PhoneNumber().fake(),
        "phonenumber.cellphone" | "phonenumber.cellnumber" => CellNumber().fake(),
        "company.name" => CompanyName().fake(),
        "company.industry" => Industry().fake(),
        "company.buzzword" => Buzzword().fake(),
        "company.catchphrase" => CatchPhrase().fake(),
        "company.bs" => Bs().fake(),
        "company.profession" => Profession().fake(),
        "company.suffix" => CompanySuffix().fake(),
        "lorem.word" => word(),
        "lorem.words" => Words(3..6).fake::<Vec<String>>().join(" "),
        "lorem.sentence" => Sentence(4..10).fake(),
        "lorem.paragraph" => Paragraph(3..6).fake(),
        "lorem.characters" => (0..10).map(|_| fastrand::alphanumeric()).collect(),
        "job.title" => JobTitle().fake(),
        "job.position" => Position().fake(),
        "job.field" => Field().fake(),
        "job.seniority" => Seniority().fake(),
        "number.digit" | "number.randomdigit" => Digit().fake(),
        "number.randomnumber" => fastrand::u32(..1_000_000_000).to_string(),
        "bool.bool" | "boolean.bool" => Boolean(50).fake::<bool>().to_string(),
        "color.name" => Color().fake(),
        "color.hex" => HexColor().fake(),
        "color.rgb" => RgbColor().fake(),
        "finance.creditcard" | "business.creditcardnumber" => CreditCardNumber().fake(),
        "finance.bic" => Bic().fake(),
        "finance.isin" => Isin().fake(),
        "currency.code" => CurrencyCode().fake(),
        "currency.name" => CurrencyName().fake(),
        "currency.symbol" => CurrencySymbol().fake(),
        "code.isbn10" => Isbn10().fake(),
        "code.isbn13" | "code.isbn" => Isbn13().fake(),
        "file.filename" => FileName().fake(),
        "file.extension" => FileExtension().fake(),
        "file.filepath" => FilePath().fake(),
        "file.mimetype" => MimeType().fake(),
        "app.version" => Semver().fake(),
        "app.name" => format!("{} {}", cap(&word()), cap(&word())),
        "commerce.productname" => format!("{} {} {}", pick(&["Small", "Ergonomic", "Rustic", "Sleek", "Gorgeous", "Practical"]), pick(&["Steel", "Wooden", "Cotton", "Granite", "Plastic"]), pick(&["Chair", "Table", "Shoes", "Lamp", "Keyboard", "Bottle"])),
        "commerce.price" => format!("{:.2}", 1.0 + fastrand::f64() * 999.0),
        "commerce.department" => pick(&["Books", "Electronics", "Garden", "Grocery", "Health", "Home", "Kids", "Music", "Sports", "Toys"]),
        "commerce.color" => Color().fake(),
        "date.birthday" => {
            let days = fastrand::i64(18 * 365..80 * 365);
            (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
        }
        "date.past" => (chrono::Utc::now() - chrono::Duration::seconds(fastrand::i64(60..365 * 86_400))).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "date.future" => (chrono::Utc::now() + chrono::Duration::seconds(fastrand::i64(60..365 * 86_400))).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "idnumber.valid" => format!("{:03}-{:02}-{:04}", fastrand::u32(100..900), fastrand::u32(10..100), fastrand::u32(1000..10_000)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_key_works() {
        for key in KEYS {
            let v = fake_value(key).unwrap_or_else(|| panic!("{key} not handled"));
            assert!(!v.is_empty(), "{key} produced an empty value");
        }
        assert!(fake_value("name.first_name").is_some());
        assert!(fake_value("Unknown.key").is_none());
    }
}
