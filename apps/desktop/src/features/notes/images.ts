import { notesApi } from '@/lib/api/notes';

const MIME: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  svg: 'image/svg+xml',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  avif: 'image/avif',
};

/**
 * Where an image a note shows lives, relative to `notes/`: `img/a.png` next to the note,
 * `../shared/b.png`, `notes/…` or `/notes/…` from the project root. Null for URLs and anything
 * outside `notes/` (notes only read images from their own folder).
 */
export function noteImagePath(src: string, notePath: string): string | null {
  let s = src.trim().split(/[?#]/)[0];
  try {
    s = decodeURIComponent(s);
  } catch {
    /* keep as written */
  }
  s = s.replace(/\\/g, '/');
  if (!s || /^[a-z][a-z0-9+.-]*:/i.test(s)) return null;
  let parts: string[];
  if (s.startsWith('/')) {
    if (!s.startsWith('/notes/')) return null;
    parts = s.slice(7).split('/');
  } else if (s.startsWith('notes/')) parts = s.slice(6).split('/');
  else parts = [...notePath.split('/').slice(0, -1), ...s.split('/')];
  const out: string[] = [];
  for (const p of parts) {
    if (!p || p === '.') continue;
    if (p === '..') {
      if (!out.length) return null;
      out.pop();
    } else out.push(p);
  }
  const path = out.join('/');
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  return path && MIME[ext] ? path : null;
}

/** Blob URLs of loaded note images (a small cache; the oldest are released). */
const urls = new Map<string, Promise<string | null>>();

/** A displayable URL for an image in a note (null when it isn't in `notes/` or can't be read). */
export function noteImageUrl(root: string, notePath: string, src: string): Promise<string | null> {
  const path = noteImagePath(src, notePath);
  if (!path) return Promise.resolve(null);
  const key = `${root}|${path}`;
  let p = urls.get(key);
  if (!p) {
    const mime = MIME[path.split('.').pop()!.toLowerCase()];
    p = notesApi
      .asset(root, path)
      .then((buf) => URL.createObjectURL(new Blob([buf], { type: mime })))
      .catch(() => {
        urls.delete(key);
        return null;
      });
    urls.set(key, p);
    if (urls.size > 80) {
      const [oldKey, old] = urls.entries().next().value!;
      urls.delete(oldKey);
      void old.then((u) => u && URL.revokeObjectURL(u));
    }
  }
  return p;
}
