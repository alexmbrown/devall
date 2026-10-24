// Environment chip colours: a preset keyword (themed via CSS) or any custom colour (hex from the
// picker; imported files may carry CSS colour names). Pure — unit-tested in envColor.test.ts.

export const ENV_PRESETS = ['local', 'dev', 'test', 'staging', 'prod'] as const;
export type EnvPreset = (typeof ENV_PRESETS)[number];

/** Sample colours of the presets (light theme), to seed the custom picker. */
export const PRESET_HEX: Record<EnvPreset, string> = {
  local: '#6b7280',
  dev: '#16a34a',
  test: '#0891b2',
  staging: '#d97706',
  prod: '#dc2626',
};

export const isPreset = (c: string | undefined | null): c is EnvPreset => !!c && (ENV_PRESETS as readonly string[]).includes(c);
export const isHexColor = (c: string | undefined | null): c is string => !!c && /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(c.trim());

/** `#abc` → `#aabbcc`, lower-case; anything else → null. */
export function normalizeHex(c: string): string | null {
  const v = c.trim().replace(/^([0-9a-f]{3}|[0-9a-f]{6})$/i, '#$1');
  if (!isHexColor(v)) return null;
  const h = v.slice(1).toLowerCase();
  return `#${h.length === 3 ? [...h].map((x) => x + x).join('') : h}`;
}

/** A CSS colour we can paint with: hex always; names/rgb() when the browser accepts them. */
function cssColor(c: string): string | null {
  if (isHexColor(c)) return c.trim();
  const ok = typeof CSS !== 'undefined' && typeof CSS.supports === 'function' && CSS.supports('color', c);
  return ok ? c : null;
}

export interface ChipInfo {
  /** Chip text; undefined → render a colour dot only. */
  text?: string;
  /** Preset class (`env-staging`), when the colour is a preset. */
  preset?: EnvPreset;
  /** Custom colour for `--env-c`. */
  color?: string;
}

/** How an environment's chip looks, or null when it has neither colour nor label. */
export function envChipInfo(env: { color?: string; label?: string }): ChipInfo | null {
  const label = env.label?.trim() || undefined;
  const c = env.color?.trim();
  if (c && isPreset(c)) return { preset: c, text: label ?? c };
  const color = c ? cssColor(c) : null;
  if (color) return { color, text: label };
  return label ? { text: label } : null;
}
