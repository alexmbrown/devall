/** `transformerParameters.dataSource` as the /dev/all engine reads it (crates/devall-mock data.rs). */
export interface DataLookup {
  name: string;
  where?: Record<string, string>;
  limit?: number;
  notFoundStatus?: number;
  sql?: string;
  params?: string[];
}

/** A Handlebars argument: bare for paths/sub-expressions, quoted for plain text. */
function arg(expr: string): string {
  const e = expr.trim();
  if (!e) return "''";
  if (/^\(.*\)$/.test(e) || /^(request|parameters|row|data)\b/.test(e) || /^-?\d+(\.\d+)?$/.test(e)) return e;
  return `'${e.replace(/'/g, "\\'")}'`;
}

function quoted(s: string): string {
  return `'${s.replace(/'/g, "\\'")}'`;
}

/** `{{dataSource 'users' 'id' request.path.id}}` */
export function dataSourceSnippet(name: string, column: string, expr: string): string {
  if (!column) return `{{dataRows ${quoted(name)}}}`;
  return `{{dataSource ${quoted(name)} ${quoted(column)} ${arg(expr)}}}`;
}

/** The lookup config: `where` values are templates (`{{request.path.id}}`). */
export function lookupConfig(name: string, column: string, expr: string, notFound404: boolean): DataLookup {
  const e = expr.trim();
  const template = /^\{\{.*\}\}$/.test(e) ? e : `{{${e}}}`;
  const cfg: DataLookup = { name };
  if (column) cfg.where = { [column]: template };
  if (notFound404) cfg.notFoundStatus = 404;
  return cfg;
}
