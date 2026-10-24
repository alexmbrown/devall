/**
 * Keys understood by the built-in engine's `{{random 'Category.key'}}` helper (Datafaker-style;
 * mirrors `KEYS` in crates/devall-mock/src/template/faker.rs — keep in sync). WireMock itself
 * needs the faker extension for `random`; the other helpers below are WireMock built-ins.
 */
export const FAKER_KEYS = [
  'Name.firstName',
  'Name.lastName',
  'Name.fullName',
  'Name.name',
  'Name.nameWithMiddle',
  'Name.prefix',
  'Name.title',
  'Name.suffix',
  'Name.username',
  'Internet.emailAddress',
  'Internet.safeEmailAddress',
  'Internet.domainName',
  'Internet.domainSuffix',
  'Internet.url',
  'Internet.ipV4Address',
  'Internet.ipV6Address',
  'Internet.macAddress',
  'Internet.password',
  'Internet.userAgent',
  'Internet.uuid',
  'Internet.slug',
  'Address.city',
  'Address.streetAddress',
  'Address.streetName',
  'Address.buildingNumber',
  'Address.secondaryAddress',
  'Address.zipCode',
  'Address.postcode',
  'Address.country',
  'Address.countryCode',
  'Address.state',
  'Address.stateAbbr',
  'Address.latitude',
  'Address.longitude',
  'Address.timeZone',
  'Address.fullAddress',
  'PhoneNumber.phoneNumber',
  'PhoneNumber.cellPhone',
  'Company.name',
  'Company.industry',
  'Company.buzzword',
  'Company.catchPhrase',
  'Company.bs',
  'Company.profession',
  'Company.suffix',
  'Company.url',
  'Lorem.word',
  'Lorem.words',
  'Lorem.sentence',
  'Lorem.paragraph',
  'Lorem.characters',
  'Job.title',
  'Job.position',
  'Job.field',
  'Job.seniority',
  'Number.digit',
  'Number.randomDigit',
  'Number.randomNumber',
  'Bool.bool',
  'Color.name',
  'Color.hex',
  'Color.rgb',
  'Finance.creditCard',
  'Finance.bic',
  'Finance.isin',
  'Currency.code',
  'Currency.name',
  'Currency.symbol',
  'Code.isbn10',
  'Code.isbn13',
  'File.fileName',
  'File.extension',
  'File.filePath',
  'File.mimeType',
  'App.version',
  'App.name',
  'Commerce.productName',
  'Commerce.price',
  'Commerce.department',
  'Commerce.color',
  'Date.birthday',
  'Date.past',
  'Date.future',
  'IdNumber.valid',
] as const;

export type FakerKey = (typeof FAKER_KEYS)[number];

/** Faker keys grouped by category, in engine order. */
export function fakerGroups(): { group: string; keys: string[] }[] {
  const out: { group: string; keys: string[] }[] = [];
  for (const k of FAKER_KEYS) {
    const g = k.split('.')[0];
    const last = out[out.length - 1];
    if (last?.group === g) last.keys.push(k);
    else out.push({ group: g, keys: [k] });
  }
  return out;
}

export function fakerHelper(key: string): string {
  return `{{random '${key}'}}`;
}

/** WireMock built-in random helpers (no extension needed). */
export const RANDOM_HELPERS: { label: string; snippet: string }[] = [
  { label: 'UUID', snippet: "{{randomValue type='UUID'}}" },
  { label: 'Alphanumeric (12)', snippet: "{{randomValue length=12 type='ALPHANUMERIC'}}" },
  { label: 'Digits (6)', snippet: "{{randomValue length=6 type='NUMERIC'}}" },
  { label: 'Hex (8)', snippet: "{{randomValue length=8 type='HEXADECIMAL'}}" },
  { label: 'Integer 1–100', snippet: '{{randomInt lower=1 upper=100}}' },
  { label: 'Decimal 0–100', snippet: '{{randomDecimal lower=0 upper=100}}' },
  { label: 'One of…', snippet: "{{pickRandom 'a' 'b' 'c'}}" },
  { label: 'Now (ISO)', snippet: '{{now}}' },
  { label: 'Today', snippet: "{{now format='yyyy-MM-dd'}}" },
  { label: 'Epoch ms', snippet: "{{now format='epoch'}}" },
];
