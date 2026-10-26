import type { ReactNode } from 'react';

/**
 * React content rendered into DOM nodes owned by ProseMirror node views (chips, code block bars,
 * images). Node views register their container here; the editor component renders a portal per
 * entry, so the content lives in the app's React tree (stores, hooks, context).
 */
export class Portals {
  private items = new Map<HTMLElement, { key: number; node: ReactNode }>();
  private listeners = new Set<() => void>();
  private seq = 0;
  private version = 0;
  private scheduled = false;

  set(el: HTMLElement, node: ReactNode) {
    const cur = this.items.get(el);
    this.items.set(el, { key: cur?.key ?? ++this.seq, node });
    this.changed();
  }

  delete(el: HTMLElement) {
    if (this.items.delete(el)) this.changed();
  }

  entries() {
    return [...this.items.entries()];
  }

  subscribe = (f: () => void) => {
    this.listeners.add(f);
    return () => void this.listeners.delete(f);
  };

  snapshot = () => this.version;

  private changed() {
    this.version++;
    // Node views are created in bursts (a whole document): notify once.
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => {
      this.scheduled = false;
      for (const f of this.listeners) f();
    });
  }
}
