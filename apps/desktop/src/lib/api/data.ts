import { invoke } from '@tauri-apps/api/core';

// Mirrors of crates/devall-data (model.rs, store.rs, output.rs) and the commands in
// src-tauri/src/modules/data.rs. Keep in sync.

export type TimestampUnit = 's' | 'ms';
export type SqlDialect = 'postgres' | 'mysql' | 'sqlite' | 'sqlserver';

/** A field generator: `type` plus its parameters (all optional ones fall back to defaults). */
export type FieldKind =
  // ids
  | { type: 'uuid'; version?: 4 | 7 }
  | { type: 'ulid' }
  | { type: 'sequence'; start?: number; step?: number }
  // person
  | { type: 'firstName' }
  | { type: 'lastName' }
  | { type: 'fullName' }
  | { type: 'username' }
  | { type: 'email'; domain?: string }
  | { type: 'phone' }
  // internet
  | { type: 'url' }
  | { type: 'domain' }
  | { type: 'ipv4' }
  | { type: 'ipv6' }
  | { type: 'userAgent' }
  | { type: 'password'; minLength?: number; maxLength?: number }
  // address
  | { type: 'street' }
  | { type: 'city' }
  | { type: 'state' }
  | { type: 'zip' }
  | { type: 'country' }
  | { type: 'countryCode' }
  | { type: 'latitude' }
  | { type: 'longitude' }
  // company
  | { type: 'company' }
  | { type: 'jobTitle' }
  | { type: 'catchPhrase' }
  // commerce
  | { type: 'productName' }
  | { type: 'price'; min?: number; max?: number; decimals?: number }
  | { type: 'currency' }
  // text
  | { type: 'word' }
  | { type: 'sentence'; minWords?: number; maxWords?: number }
  | { type: 'paragraph'; sentences?: number }
  | { type: 'lorem'; words?: number }
  // numbers
  | { type: 'int'; min?: number; max?: number }
  | { type: 'float'; min?: number; max?: number; decimals?: number }
  | { type: 'bool'; truePercent?: number }
  // dates
  | { type: 'date'; from?: string; to?: string; format?: string }
  | { type: 'datetime'; from?: string; to?: string; format?: string }
  | { type: 'timestamp'; from?: string; to?: string; unit?: TimestampUnit }
  // logic & structure
  | { type: 'enum'; values: unknown[]; weights?: number[] }
  | { type: 'regex'; pattern: string }
  | { type: 'template'; value: string }
  | { type: 'ref'; entity: string; field: string }
  | { type: 'object'; fields: DataField[] }
  | { type: 'array'; of: FieldKind; min?: number; max?: number }
  | { type: 'json'; value: unknown };

export type FieldType = FieldKind['type'];

/** A column: name + generator (flattened, as in the YAML file) + null rate + uniqueness. */
export type DataField = FieldKind & {
  name: string;
  /** 0–100. */
  nullPercent?: number;
  unique?: boolean;
};

export interface DataEntity {
  name: string;
  count: number;
  fields: DataField[];
  [extra: string]: unknown;
}

/** `<project>/data/<slug>.data.yaml`. */
export interface DataSchema {
  version?: number;
  name: string;
  /** Keep ≤ 2^53 so it survives JS numbers. Omitted = random per run. */
  seed?: number;
  locale?: string;
  entities: DataEntity[];
  [extra: string]: unknown;
}

export type OutputFormat =
  | { type: 'json'; pretty?: boolean }
  | { type: 'ndjson' }
  | { type: 'csv'; delimiter?: string; header?: boolean }
  | { type: 'sql'; dialect: SqlDialect; createTable?: boolean; batchSize?: number }
  | { type: 'yaml' };

export interface DataFileEntry {
  /** Relative to the project's `data/` folder. */
  path: string;
  name: string;
  entities: string[];
  error?: string;
}

export interface ExportResult {
  files: string[];
  rows: number;
  bytes: number;
  seed: number;
}

export interface GenerateArgs {
  root?: string;
  schema: DataSchema;
  /** Emit only this entity (its parents are still generated for foreign keys). */
  entity?: string;
  countOverride?: number;
  /** Rows per entity (capped at 1000 by the backend). */
  limit?: number;
  format: OutputFormat;
}

/** Typed wrappers over the `data_*` Tauri commands. Errors reject with a string. */
export const dataApi = {
  list: (root: string) => invoke<DataFileEntry[]>('data_list', { root }),
  read: (root: string, path: string) => invoke<DataSchema>('data_read', { root, path }),
  save: (root: string, path: string, schema: DataSchema) => invoke<void>('data_save', { root, path, schema }),
  /** Creates `data/<slug>.data.yaml` (from `schema`, else a starter schema); returns its path. */
  create: (root: string, name: string, schema?: DataSchema) => invoke<string>('data_create', { root, name, schema }),
  remove: (root: string, path: string) => invoke<void>('data_delete', { root, path }),
  generate: ({ root, schema, entity, countOverride, limit, format }: GenerateArgs) =>
    invoke<string>('data_generate', { root, schema, entity, countOverride, limit, format }),
  exportFile: (root: string, path: string, format: OutputFormat, destPath: string, entity?: string, countOverride?: number) =>
    invoke<ExportResult>('data_export', { root, path, format, destPath, entity, countOverride }),
  infer: (jsonText: string, entity?: string) => invoke<DataSchema>('data_infer', { jsonText, entity }),
  fromJsonSchema: (text: string) => invoke<DataSchema>('data_from_json_schema', { text }),
  toTemplate: (jsonText: string, keepIds = false) => invoke<string>('data_to_template', { jsonText, keepIds }),
  templateSample: (template: string, seed?: number) => invoke<string>('data_template_sample', { template, seed }),
};

/** `(code, label)` of every supported locale (crates/devall-data/src/fakers.rs `LOCALES`). */
export const DATA_LOCALES: [string, string][] = [
  ['en', 'English'],
  ['fr_fr', 'French'],
  ['de_de', 'German'],
  ['it_it', 'Italian'],
  ['pt_br', 'Portuguese (Brazil)'],
  ['pt_pt', 'Portuguese (Portugal)'],
  ['nl_nl', 'Dutch'],
  ['ja_jp', 'Japanese'],
  ['zh_cn', 'Chinese (Simplified)'],
  ['zh_tw', 'Chinese (Traditional)'],
  ['ar_sa', 'Arabic'],
  ['tr_tr', 'Turkish'],
  ['fa_ir', 'Persian'],
  ['cy_gb', 'Welsh'],
];
