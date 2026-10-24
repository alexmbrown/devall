// Client-side mirror of crates/devall-proxy/src/filter.rs (keep the two in sync), so the live
// list filters instantly without a round trip.
import type { ContentKind, ExchangeFilter, ExchangeSummary } from '@/lib/api/proxy';

export function contentKind(contentType: string | null | undefined, path: string): ContentKind {
  const ct = (contentType ?? '').toLowerCase().split(';')[0].trim();
  if (ct) {
    if (ct.includes('json')) return 'json';
    if (ct.includes('html')) return 'html';
    if (ct.includes('javascript') || ct.includes('ecmascript') || ct === 'application/wasm') return 'script';
    if (ct === 'text/css') return 'css';
    if (ct.startsWith('image/')) return 'image';
    if (ct.startsWith('font/') || ct.includes('font-') || ct.includes('woff')) return 'font';
    if (ct.startsWith('audio/') || ct.startsWith('video/')) return 'media';
    if (ct.endsWith('xml')) return 'xml';
    if (ct.startsWith('text/')) return 'text';
  }
  const p = path.split(/[?#]/)[0].toLowerCase();
  const dot = p.lastIndexOf('.');
  const ext = dot >= 0 && !p.slice(dot + 1).includes('/') ? p.slice(dot + 1) : '';
  switch (ext) {
    case 'js':
    case 'mjs':
    case 'cjs':
    case 'map':
    case 'wasm':
      return 'script';
    case 'css':
      return 'css';
    case 'png':
    case 'jpg':
    case 'jpeg':
    case 'gif':
    case 'webp':
    case 'avif':
    case 'svg':
    case 'ico':
    case 'bmp':
      return 'image';
    case 'woff':
    case 'woff2':
    case 'ttf':
    case 'otf':
    case 'eot':
      return 'font';
    case 'mp4':
    case 'webm':
    case 'mp3':
    case 'ogg':
    case 'wav':
    case 'm4a':
    case 'm3u8':
    case 'ts':
      return 'media';
    case 'html':
    case 'htm':
      return 'html';
    case 'json':
      return 'json';
    case 'xml':
      return 'xml';
    default:
      return 'other';
  }
}

export function kindOf(s: ExchangeSummary): ContentKind {
  if (s.state === 'webSocket' || s.wsFrames > 0) return 'webSocket';
  return contentKind(s.contentType, s.path);
}

export const isStatic = (k: ContentKind) => k === 'script' || k === 'css' || k === 'image' || k === 'font' || k === 'media';

export const KIND_LABEL: Record<ContentKind, string> = {
  json: 'JSON',
  html: 'HTML',
  script: 'JS',
  css: 'CSS',
  image: 'Image',
  font: 'Font',
  media: 'Media',
  xml: 'XML',
  text: 'Text',
  webSocket: 'WS',
  other: 'Other',
};

export function matches(f: ExchangeFilter, s: ExchangeSummary): boolean {
  if (f.methods?.length && !f.methods.some((m) => m.toUpperCase() === s.method.toUpperCase())) return false;
  if (f.statusClasses?.length) {
    const cls = s.status == null ? 0 : Math.floor(s.status / 100);
    if (!f.statusClasses.includes(cls)) return false;
  }
  if (f.host?.trim() && !s.host.toLowerCase().includes(f.host.trim().toLowerCase())) return false;
  const kind = kindOf(s);
  if (f.contentKinds?.length && !f.contentKinds.includes(kind)) return false;
  if (f.hideStatic && isStatic(kind)) return false;
  if (f.onlyPaused && !s.paused) return false;
  const terms = (f.text ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length) {
    const hay = `${s.method} ${s.url} ${s.status ?? ''}`.toLowerCase();
    for (const t of terms) {
      if (t.startsWith('-') && t.length > 1) {
        if (hay.includes(t.slice(1))) return false;
      } else if (!hay.includes(t)) return false;
    }
  }
  return true;
}

export function isEmptyFilter(f: ExchangeFilter): boolean {
  return !f.text?.trim() && !f.methods?.length && !f.statusClasses?.length && !f.host?.trim() && !f.contentKinds?.length && !f.hideStatic && !f.onlyPaused;
}
