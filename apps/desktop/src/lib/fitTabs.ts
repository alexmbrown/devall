/**
 * Picks how a row of sub-tabs fits a width without scrolling: full labels, tighter padding, icons
 * only, and finally icons with the rest in a "More" menu (the active tab always stays visible).
 * With `prefer: 'labels'` (long rows, e.g. the mock server) it keeps labels and moves what doesn't
 * fit into "More" instead of collapsing every tab to an icon.
 */
export type TabPreference = 'icons' | 'labels';
export type TabMode = 'full' | 'compact' | 'icon';

export interface TabWidths {
  full: number[];
  compact: number[];
  icon: number[];
}

export interface TabFit {
  mode: TabMode;
  /** Indices shown in the row, in order. */
  visible: number[];
  /** Indices moved to the "More" menu. */
  overflow: number[];
}

const rowWidth = (w: number[], gap: number) => (w.length ? w.reduce((a, b) => a + b, 0) + gap * (w.length - 1) : 0);

export function fitTabs(widths: TabWidths, available: number, active: number, moreWidth: number, gap = 0, prefer: TabPreference = 'icons'): TabFit {
  const n = widths.full.length;
  const all = Array.from({ length: n }, (_, i) => i);
  // Not measured yet: show everything.
  if (available <= 0 || n === 0) return { mode: 'full', visible: all, overflow: [] };
  for (const mode of ['full', 'compact', 'icon'] as const) {
    if (prefer === 'labels' && mode === 'icon') break;
    if (rowWidth(widths[mode], gap) <= available) return { mode, visible: all, overflow: [] };
  }
  if (prefer === 'labels') {
    const fit = overflowFit(widths.compact, all, available, active, moreWidth, gap);
    // Labels are only worth it while at least a few fit; otherwise fall back to icons.
    if (fit.visible.length >= Math.min(3, n)) return { mode: 'compact', ...fit };
  }
  return { mode: 'icon', ...overflowFit(widths.icon, all, available, active, moreWidth, gap) };
}

/** Keeps tabs in order while they fit next to the More button; the active tab always stays. */
function overflowFit(w: number[], all: number[], available: number, active: number, moreWidth: number, gap: number): { visible: number[]; overflow: number[] } {
  const n = all.length;
  const room = available - moreWidth - gap;
  const visible: number[] = [];
  let used = 0;
  for (const i of all) {
    const wi = w[i] + (visible.length ? gap : 0);
    if (used + wi > room) break;
    visible.push(i);
    used += wi;
  }
  if (active >= 0 && active < n && !visible.includes(active)) {
    // Make room for the active tab by dropping the last visible ones.
    while (visible.length && used + gap + w[active] > room) {
      const last = visible.pop()!;
      used -= w[last] + (visible.length ? gap : 0);
    }
    visible.push(active);
    visible.sort((a, b) => a - b);
  }
  return { visible, overflow: all.filter((i) => !visible.includes(i)) };
}
