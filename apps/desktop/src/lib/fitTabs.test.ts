import { describe, expect, it } from 'vitest';
import { fitTabs, type TabWidths } from './fitTabs';

const W: TabWidths = {
  full: [80, 90, 70, 60],
  compact: [70, 80, 60, 50],
  icon: [40, 40, 40, 40],
};

describe('fitTabs', () => {
  it('shows full labels when they fit', () => {
    expect(fitTabs(W, 400, 0, 30, 2)).toEqual({ mode: 'full', visible: [0, 1, 2, 3], overflow: [] });
  });

  it('tightens padding, then drops to icons', () => {
    expect(fitTabs(W, 270, 0, 30, 2).mode).toBe('compact');
    expect(fitTabs(W, 200, 0, 30, 2)).toEqual({ mode: 'icon', visible: [0, 1, 2, 3], overflow: [] });
  });

  it('moves what does not fit into the More menu, keeping the active tab visible', () => {
    // room = 120 - 30 - 2 = 88 → two icons (40 + 2 + 40).
    expect(fitTabs(W, 120, 0, 30, 2)).toEqual({ mode: 'icon', visible: [0, 1], overflow: [2, 3] });
    expect(fitTabs(W, 120, 3, 30, 2)).toEqual({ mode: 'icon', visible: [0, 3], overflow: [1, 2] });
  });

  it('shows everything until measured', () => {
    expect(fitTabs(W, 0, 0, 30).overflow).toEqual([]);
    expect(fitTabs({ full: [], compact: [], icon: [] }, 100, 0, 30).visible).toEqual([]);
  });

  it('keeps the active tab even when nothing else fits', () => {
    expect(fitTabs(W, 50, 2, 30, 2)).toEqual({ mode: 'icon', visible: [2], overflow: [0, 1, 3] });
  });

  it('prefers labels with a More menu over icons for long rows', () => {
    const widths = { full: [100, 100, 100, 100, 100], compact: [90, 90, 90, 90, 90], icon: [30, 30, 30, 30, 30] };
    const fit = fitTabs(widths, 320, 4, 34, 0, 'labels');
    expect(fit.mode).toBe('compact');
    expect(fit.visible).toEqual([0, 1, 4]);
    expect(fit.overflow).toEqual([2, 3]);
    // Default preference collapses to icons, which all fit.
    expect(fitTabs(widths, 320, 4, 34, 0).mode).toBe('icon');
  });

  it('falls back to icons when fewer than three labels fit', () => {
    const widths = { full: [100, 100, 100, 100, 100], compact: [90, 90, 90, 90, 90], icon: [30, 30, 30, 30, 30] };
    expect(fitTabs(widths, 200, 0, 34, 0, 'labels').mode).toBe('icon');
  });
});
