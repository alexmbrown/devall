import { ICON, MARK } from '@/components/brandMark';
import { isHexColor, normalizeHex } from '@/lib/envColor';

/**
 * The accent colour: one colour (Settings → Appearance) drives the UI accent tokens, the logo and
 * the window/taskbar icon. `null` keeps the built-in palette (tokens.css), which this default
 * reproduces.
 */
export const DEFAULT_ACCENT = '#4f46e5';

export const ACCENT_PRESETS: { name: string; hex: string }[] = [
  { name: 'Indigo', hex: '#4f46e5' },
  { name: 'Violet', hex: '#7c3aed' },
  { name: 'Blue', hex: '#2563eb' },
  { name: 'Sky', hex: '#0284c7' },
  { name: 'Teal', hex: '#0d9488' },
  { name: 'Green', hex: '#16a34a' },
  { name: 'Amber', hex: '#d97706' },
  { name: 'Orange', hex: '#ea580c' },
  { name: 'Red', hex: '#dc2626' },
  { name: 'Rose', hex: '#e11d48' },
  { name: 'Pink', hex: '#db2777' },
  { name: 'Slate', hex: '#475569' },
];

type Rgb = [number, number, number];

const WHITE: Rgb = [255, 255, 255];
const BLACK: Rgb = [0, 0, 0];
/** tokens.css dark `--bg`, for the subtle/border shades that sit on dark surfaces. */
const DARK_BG: Rgb = [14, 14, 17];

function rgb(hex: string): Rgb {
  const h = (normalizeHex(hex) ?? DEFAULT_ACCENT).slice(1);
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

const toHex = (c: Rgb) => `#${c.map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0')).join('')}`;

/** `t` of the way from `a` to `b`. */
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];

function luminance([r, g, b]: Rgb): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** White or near-black, whichever reads better on `hex`. */
export function readableText(hex: string): string {
  const l = luminance(rgb(hex));
  // WCAG contrast against white vs against black.
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.05 ? '#ffffff' : '#111111';
}

/** The accent token set for `hex` in a theme (mirrors how tokens.css derives the default indigo). */
export function accentTokens(hex: string, theme: 'light' | 'dark'): Record<string, string> {
  const c = rgb(hex);
  if (theme === 'light') {
    return {
      '--accent': toHex(c),
      '--accent-hover': toHex(mix(c, BLACK, 0.12)),
      '--accent-subtle': toHex(mix(c, WHITE, 0.9)),
      '--accent-border': toHex(mix(c, WHITE, 0.7)),
      '--accent-fg': toHex(mix(c, BLACK, 0.12)),
      '--accent-text': readableText(toHex(c)),
    };
  }
  const base = mix(c, WHITE, 0.15);
  return {
    '--accent': toHex(base),
    '--accent-hover': toHex(mix(c, WHITE, 0.28)),
    '--accent-subtle': toHex(mix(c, DARK_BG, 0.8)),
    '--accent-border': toHex(mix(c, DARK_BG, 0.55)),
    '--accent-fg': toHex(mix(c, WHITE, 0.45)),
    '--accent-text': readableText(toHex(base)),
  };
}

export const ACCENT_TOKEN_NAMES = Object.keys(accentTokens(DEFAULT_ACCENT, 'light'));

export function validAccent(hex: string | null | undefined): string | null {
  return isHexColor(hex) ? normalizeHex(hex) : null;
}

const loadImage = (src: string) =>
  new Promise<HTMLImageElement>((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('could not load the icon'));
    img.src = src;
  });

/** The app icon (the <D/A> mark on its tile, as in public/brand/icon.svg) in `hex`, as raw RGBA. */
export async function renderAppIcon(hex: string, size = 256): Promise<{ rgba: Uint8Array; size: number }> {
  const tile = ICON.size - 2 * ICON.inset;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${ICON.size} ${ICON.size}" width="${size}" height="${size}">` +
    `<rect x="${ICON.inset}" y="${ICON.inset}" width="${tile}" height="${tile}" rx="${ICON.radius}" fill="${hex}"/>` +
    `<path transform="translate(${ICON.markX} ${ICON.markY}) scale(${ICON.markScale})" fill="${readableText(hex)}" fill-rule="evenodd" d="${MARK.path}"/></svg>`;
  const img = await loadImage(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`);
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = size;
  const g = canvas.getContext('2d')!;
  g.drawImage(img, 0, 0, size, size);
  return { rgba: new Uint8Array(g.getImageData(0, 0, size, size).data.buffer), size };
}
