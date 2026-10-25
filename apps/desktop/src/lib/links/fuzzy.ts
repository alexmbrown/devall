import type { ReactNode } from 'react';
import { KIND_META, LINK_KINDS, type LinkKind } from './syntax';

/** Something the "Link to…" picker offers (see `LinkProvider.items`). */
export interface LinkItem {
  link: string;
  kind: LinkKind;
  title: string;
  /** Path, id or context (matched too, shown dimmed). */
  detail?: string;
  /** Extra words that match (e.g. a request's method). */
  keywords?: string;
  /** Icon for the row (method badge, environment colour…); a default per kind otherwise. */
  icon?: ReactNode;
}

export interface LinkGroup<T extends LinkItem = LinkItem> {
  kind: LinkKind;
  /** Group heading (kinds sharing a heading are merged, e.g. environments + project variables). */
  label: string;
  items: T[];
}

/**
 * How well `needle` (lower case) matches `hay`: −1 for no match. Substrings win (more at the start
 * and at word starts), then in-order subsequences (fewer, shorter gaps score higher).
 */
export function fuzzyScore(needle: string, hay: string): number {
  if (!needle) return 0;
  const h = hay.toLowerCase();
  const at = h.indexOf(needle);
  if (at >= 0) {
    const wordStart = at === 0 || /[\s/._\-:([]/.test(h[at - 1]);
    return 100 + (at === 0 ? 40 : 0) + (wordStart ? 20 : 0) - Math.min(at, 30) + (needle.length === h.length ? 30 : 0);
  }
  let score = 40;
  let pos = -1;
  for (const ch of needle) {
    const next = h.indexOf(ch, pos + 1);
    if (next < 0) return -1;
    if (pos >= 0 && next > pos + 1) score -= Math.min(6, next - pos - 1);
    else if (pos >= 0) score += 2;
    pos = next;
  }
  return Math.max(1, score);
}

/** A leading `alias:` narrows to one kind: `todo: refunds`, `note:auth`. */
export function parseQuery(query: string): { kinds?: LinkKind[]; text: string } {
  // Not `scheme://…`: a URL typed into the to-do "Add link…" picker stays text.
  const m = /^\s*([a-z-]+):(?!\/\/)\s*(.*)$/i.exec(query);
  if (m) {
    const alias = m[1].toLowerCase();
    const kinds = LINK_KINDS.filter((k) => KIND_META[k].aliases.includes(alias));
    if (kinds.length) return { kinds, text: m[2].trim() };
  }
  return { text: query.trim() };
}

function itemScore(item: LinkItem, tokens: string[]): number {
  let total = 0;
  const label = KIND_META[item.kind].label;
  for (const t of tokens) {
    const best = Math.max(fuzzyScore(t, item.title) * 3, fuzzyScore(t, item.detail ?? ''), fuzzyScore(t, item.keywords ?? ''), fuzzyScore(t, label) * 0.5);
    if (best < 0) return -1;
    total += best;
  }
  return total;
}

/**
 * The picker's results: items matching every word of the query (title, detail, keywords, kind),
 * best first within each kind, grouped in a fixed kind order (so groups don't jump while typing). An empty query shows the first items of
 * each kind. `perGroup` caps each group, `kinds` limits the kinds.
 */
export function filterLinkItems<T extends LinkItem>(items: T[], query: string, opts: { perGroup?: number; kinds?: LinkKind[] } = {}): LinkGroup<T>[] {
  const q = parseQuery(query);
  const kinds = q.kinds && opts.kinds ? q.kinds.filter((k) => opts.kinds!.includes(k)) : (q.kinds ?? opts.kinds);
  const tokens = q.text.toLowerCase().split(/\s+/).filter(Boolean);
  const perGroup = opts.perGroup ?? (tokens.length || q.kinds ? 12 : 4);
  const scored = new Map<LinkKind, { item: T; score: number; i: number }[]>();
  items.forEach((item, i) => {
    if (kinds && !kinds.includes(item.kind)) return;
    const score = tokens.length ? itemScore(item, tokens) : 0;
    if (score < 0) return;
    const list = scored.get(item.kind) ?? [];
    list.push({ item, score, i });
    scored.set(item.kind, list);
  });
  const groups: LinkGroup<T>[] = [];
  for (const kind of LINK_KINDS) {
    const list = scored.get(kind);
    if (!list?.length) continue;
    list.sort((a, b) => b.score - a.score || a.item.title.length - b.item.title.length || a.i - b.i);
    const label = KIND_META[kind].group;
    const top = list.slice(0, perGroup).map((x) => x.item);
    const same = groups.find((g) => g.label === label);
    if (same) same.items.push(...top);
    else groups.push({ kind, label, items: top });
  }
  return groups;
}
