/**
 * Minimal list virtualisation (the part of TanStack Virtual's `useVirtualizer` the stub list and
 * request journal need): rows have a known height per index, only the visible window plus
 * `overscan` rows are rendered. `measureElement` is accepted for API compatibility; heights come
 * from `estimateSize`, so rows must render at exactly that height.
 */
import { useCallback, useLayoutEffect, useRef, useState } from 'react';

export interface VirtualItem {
  index: number;
  key: number;
  start: number;
  size: number;
  end: number;
}

export interface VirtualizerOptions {
  count: number;
  getScrollElement: () => HTMLElement | null;
  estimateSize: (index: number) => number;
  overscan?: number;
}

/** Start offsets for `count` rows; `offsets[count]` is the total height. */
export function computeOffsets(count: number, size: (i: number) => number): number[] {
  const out = new Array<number>(count + 1);
  out[0] = 0;
  for (let i = 0; i < count; i++) out[i + 1] = out[i] + size(i);
  return out;
}

/** Index range [from, to) of rows intersecting [top, top + height), widened by `overscan`. */
export function visibleRange(offsets: number[], top: number, height: number, overscan: number): [number, number] {
  const count = offsets.length - 1;
  if (count <= 0) return [0, 0];
  // First row whose end is below `top`.
  let lo = 0;
  let hi = count - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (offsets[mid + 1] <= top) lo = mid + 1;
    else hi = mid;
  }
  let end = lo;
  const bottom = top + Math.max(height, 1);
  while (end < count && offsets[end] < bottom) end++;
  return [Math.max(0, lo - overscan), Math.min(count, end + overscan)];
}

export function useVirtualizer({ count, getScrollElement, estimateSize, overscan = 8 }: VirtualizerOptions) {
  const [view, setView] = useState({ top: 0, height: 800 });
  const elRef = useRef<HTMLElement | null>(null);
  const cleanup = useRef<(() => void) | null>(null);

  // Attach to the scroll element whenever it changes (it is usually set after the first render).
  useLayoutEffect(() => {
    const el = getScrollElement();
    if (el === elRef.current) return;
    cleanup.current?.();
    elRef.current = el;
    if (!el) return;
    const update = () => setView((v) => (v.top === el.scrollTop && v.height === el.clientHeight ? v : { top: el.scrollTop, height: el.clientHeight }));
    update();
    el.addEventListener('scroll', update, { passive: true });
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(update) : null;
    ro?.observe(el);
    cleanup.current = () => {
      el.removeEventListener('scroll', update);
      ro?.disconnect();
    };
  });
  useLayoutEffect(() => () => cleanup.current?.(), []);

  const offsets = computeOffsets(count, estimateSize);
  const [from, to] = visibleRange(offsets, view.top, view.height, overscan);
  const items: VirtualItem[] = [];
  for (let i = from; i < to; i++) items.push({ index: i, key: i, start: offsets[i], size: offsets[i + 1] - offsets[i], end: offsets[i + 1] });

  const measureElement = useCallback((_el: Element | null) => {}, []);
  const scrollToIndex = useCallback(
    (i: number) => {
      const el = elRef.current;
      if (!el || i < 0 || i >= count) return;
      const top = offsets[i];
      const bottom = offsets[i + 1];
      if (top < el.scrollTop) el.scrollTop = top;
      else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [count, offsets[count]],
  );

  return {
    getTotalSize: () => offsets[count] ?? 0,
    getVirtualItems: () => items,
    measureElement,
    scrollToIndex,
  };
}
