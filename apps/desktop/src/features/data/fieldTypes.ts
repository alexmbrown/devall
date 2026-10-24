import type { DataEntity, DataField, DataSchema, FieldKind, FieldType } from '@/lib/api/data';

export interface TypeInfo {
  type: FieldType;
  label: string;
  category: string;
  hint: string;
}

export const TYPE_CATEGORIES = ['IDs', 'Person', 'Internet', 'Address', 'Company', 'Commerce', 'Text', 'Numbers', 'Dates', 'Logic & structure'] as const;

export const FIELD_TYPES: TypeInfo[] = [
  { type: 'uuid', label: 'UUID', category: 'IDs', hint: 'Random UUID (v4, or time-ordered v7)' },
  { type: 'ulid', label: 'ULID', category: 'IDs', hint: 'Time-ordered ULID' },
  { type: 'sequence', label: 'Sequence', category: 'IDs', hint: '1, 2, 3, … (start/step)' },
  { type: 'firstName', label: 'First name', category: 'Person', hint: 'Locale-aware first name' },
  { type: 'lastName', label: 'Last name', category: 'Person', hint: 'Locale-aware last name' },
  { type: 'fullName', label: 'Full name', category: 'Person', hint: "Built from the row's first/last name fields" },
  { type: 'username', label: 'Username', category: 'Person', hint: "Built from the row's name fields" },
  { type: 'email', label: 'E-mail', category: 'Person', hint: "Built from the row's name fields; example.com/.net/.org unless a domain is set" },
  { type: 'phone', label: 'Phone', category: 'Person', hint: 'Locale-aware phone number' },
  { type: 'url', label: 'URL', category: 'Internet', hint: 'https:// URL' },
  { type: 'domain', label: 'Domain', category: 'Internet', hint: 'Domain name' },
  { type: 'ipv4', label: 'IPv4', category: 'Internet', hint: 'IPv4 address' },
  { type: 'ipv6', label: 'IPv6', category: 'Internet', hint: 'IPv6 address' },
  { type: 'userAgent', label: 'User agent', category: 'Internet', hint: 'Browser user-agent string' },
  { type: 'password', label: 'Password', category: 'Internet', hint: 'Random password (length range)' },
  { type: 'street', label: 'Street address', category: 'Address', hint: 'Number + street' },
  { type: 'city', label: 'City', category: 'Address', hint: 'City name' },
  { type: 'state', label: 'State / region', category: 'Address', hint: 'State or region name' },
  { type: 'zip', label: 'Postal code', category: 'Address', hint: 'ZIP / postal code' },
  { type: 'country', label: 'Country', category: 'Address', hint: 'Country name' },
  { type: 'countryCode', label: 'Country code', category: 'Address', hint: 'ISO 3166 alpha-2' },
  { type: 'latitude', label: 'Latitude', category: 'Address', hint: '−90 … 90' },
  { type: 'longitude', label: 'Longitude', category: 'Address', hint: '−180 … 180' },
  { type: 'company', label: 'Company', category: 'Company', hint: 'Company name' },
  { type: 'jobTitle', label: 'Job title', category: 'Company', hint: 'Job title' },
  { type: 'catchPhrase', label: 'Catch phrase', category: 'Company', hint: 'Company slogan' },
  { type: 'productName', label: 'Product name', category: 'Commerce', hint: '“Ergonomic Steel Chair”' },
  { type: 'price', label: 'Price', category: 'Commerce', hint: 'Number in a range, rounded' },
  { type: 'currency', label: 'Currency', category: 'Commerce', hint: 'ISO 4217 code' },
  { type: 'word', label: 'Word', category: 'Text', hint: 'One lorem word' },
  { type: 'sentence', label: 'Sentence', category: 'Text', hint: 'One sentence' },
  { type: 'paragraph', label: 'Paragraph', category: 'Text', hint: 'A few sentences' },
  { type: 'lorem', label: 'Lorem words', category: 'Text', hint: 'N lorem words' },
  { type: 'int', label: 'Integer', category: 'Numbers', hint: 'Integer in [min, max]' },
  { type: 'float', label: 'Decimal', category: 'Numbers', hint: 'Decimal in [min, max]' },
  { type: 'bool', label: 'Boolean', category: 'Numbers', hint: 'true / false' },
  { type: 'date', label: 'Date', category: 'Dates', hint: 'Date (strftime format)' },
  { type: 'datetime', label: 'Date-time', category: 'Dates', hint: 'ISO date-time (UTC)' },
  { type: 'timestamp', label: 'Unix timestamp', category: 'Dates', hint: 'Seconds or milliseconds' },
  { type: 'enum', label: 'One of…', category: 'Logic & structure', hint: 'Pick from a list, optionally weighted' },
  { type: 'regex', label: 'Regex', category: 'Logic & structure', hint: 'String matching a pattern' },
  { type: 'template', label: 'Template', category: 'Logic & structure', hint: '{{field}} references, filters |lower |upper |slug |alnum' },
  { type: 'ref', label: 'Reference (FK)', category: 'Logic & structure', hint: 'A value from another entity' },
  { type: 'object', label: 'Object', category: 'Logic & structure', hint: 'Nested JSON object' },
  { type: 'array', label: 'Array', category: 'Logic & structure', hint: 'JSON array of items' },
  { type: 'json', label: 'Constant (JSON)', category: 'Logic & structure', hint: 'The same value every row' },
];

export const typeInfo = (t: FieldType) => FIELD_TYPES.find((x) => x.type === t);

/** A sensible starting configuration for a type. */
export function defaultKind(type: FieldType, schema?: DataSchema, entity?: string): FieldKind {
  switch (type) {
    case 'enum':
      return { type, values: ['active', 'inactive'] };
    case 'regex':
      return { type, pattern: '[A-Z]{3}-[0-9]{4}' };
    case 'template':
      return { type, value: '{{firstName|lower}}.{{lastName|lower}}@acme.test' };
    case 'ref': {
      const target = schema?.entities.find((e) => e.name !== entity) ?? schema?.entities[0];
      return { type, entity: target?.name ?? '', field: target?.fields[0]?.name ?? 'id' };
    }
    case 'object':
      return { type, fields: [{ name: 'value', type: 'word' }] };
    case 'array':
      return { type, of: { type: 'word' } };
    case 'json':
      return { type, value: null };
    default:
      return { type } as FieldKind;
  }
}

/** Replace a field's generator, keeping name / nullPercent / unique. */
export function withKind(field: DataField, kind: FieldKind): DataField {
  const { name, nullPercent, unique } = field;
  const next = { name, ...kind } as DataField;
  if (nullPercent) next.nullPercent = nullPercent;
  if (unique) next.unique = unique;
  return next;
}

/** The generator part of a field (without name / nullPercent / unique). */
export function kindOf(field: DataField): FieldKind {
  const { name: _n, nullPercent: _p, unique: _u, ...kind } = field;
  void _n;
  void _p;
  void _u;
  return kind as FieldKind;
}

const NAME_GUESSES: [RegExp, FieldType][] = [
  [/^(id|uuid|guid)$/i, 'uuid'],
  [/^(first[_-]?name|given[_-]?name|fname)$/i, 'firstName'],
  [/^(last[_-]?name|surname|family[_-]?name|lname)$/i, 'lastName'],
  [/^(full[_-]?name|display[_-]?name|name)$/i, 'fullName'],
  [/^(user[_-]?name|login|handle|nick(name)?)$/i, 'username'],
  [/e-?mail/i, 'email'],
  [/phone|mobile|^tel$/i, 'phone'],
  [/^(url|website|homepage|link|avatar|image|photo)/i, 'url'],
  [/^(domain|host(name)?)$/i, 'domain'],
  [/^ip(v4|address)?$/i, 'ipv4'],
  [/user[_-]?agent/i, 'userAgent'],
  [/^pass(word|wd)?$/i, 'password'],
  [/^(street|address(1|line1)?)$/i, 'street'],
  [/^(city|town)$/i, 'city'],
  [/^(state|province|region)$/i, 'state'],
  [/^(zip|zip[_-]?code|post(al)?[_-]?code)$/i, 'zip'],
  [/^country[_-]?code$/i, 'countryCode'],
  [/^country$/i, 'country'],
  [/^lat(itude)?$/i, 'latitude'],
  [/^(lng|lon|long|longitude)$/i, 'longitude'],
  [/^(company|organi[sz]ation|employer)$/i, 'company'],
  [/^(job[_-]?title|position|occupation)$/i, 'jobTitle'],
  [/^(product|product[_-]?name)$/i, 'productName'],
  [/(price|amount|total|cost|balance)$/i, 'price'],
  [/^currency/i, 'currency'],
  [/^(title|subject|headline)$/i, 'sentence'],
  [/^(description|bio|about|body|content|summary|notes?|comment)$/i, 'paragraph'],
  [/^(age|count|qty|quantity)$/i, 'int'],
  [/^(is|has)[A-Z_]|^(active|enabled|verified|deleted)$/, 'bool'],
  [/(_at|At|date|Date|time|Time)$/, 'datetime'],
];

/** A likely generator for a field name (used for freshly added fields). */
export function guessType(name: string): FieldType | undefined {
  return NAME_GUESSES.find(([re]) => re.test(name.trim()))?.[1];
}

export function uniqueName(base: string, taken: string[]): string {
  if (!taken.includes(base)) return base;
  let n = 2;
  while (taken.includes(`${base}${n}`)) n++;
  return `${base}${n}`;
}

export function newEntity(name: string): DataEntity {
  return {
    name,
    count: 10,
    fields: [
      { name: 'id', type: 'sequence' },
      { name: 'name', type: 'fullName' },
    ],
  };
}

/** Parses an enum value typed in the UI: JSON when it parses (numbers, true, null, "quoted"), else text. */
export function parseLooseValue(text: string): unknown {
  const t = text.trim();
  if (t === '') return '';
  try {
    return JSON.parse(t);
  } catch {
    return t;
  }
}

export function formatLooseValue(v: unknown): string {
  return typeof v === 'string' ? v : JSON.stringify(v);
}
