import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { Ellipsis } from 'lucide-react';
import { Menu } from './Menu';
import { fitTabs, type TabPreference, type TabFit, type TabWidths } from '@/lib/fitTabs';
import { usePrefs } from '@/store/prefs';

export interface TabDef<K extends string> {
  key: K;
  label: string;
  /** Shown instead of the label when space is tight. */
  icon: ReactNode;
  /** Count or short status next to the label (kept in icon mode). */
  badge?: ReactNode;
  /** Small "has content" dot. */
  dot?: boolean;
  /** Also show the icon next to the label (status icons such as pass/fail). */
  showIcon?: boolean;
  title?: string;
}

const GAP = 2;
const MORE_W = 34;

/**
 * Sub-tab row that never scrolls: it tightens its padding, then shows icons (+ counts), then moves
 * the tabs that still don't fit into a "More" menu. Sizes follow the row's own width (splits make
 * views narrow), measured with a ResizeObserver.
 */
export function ResponsiveTabs<K extends string>({
  tabs,
  value,
  onChange,
  className,
  label,
  prefer = 'icons',
}: {
  tabs: TabDef<K>[];
  value: NoInfer<K>;
  onChange(k: NoInfer<K>): void;
  className?: string;
  label?: string;
  /** 'labels' keeps labels and moves the rest into More (long rows) instead of icons for everything. */
  prefer?: TabPreference;
}) {
  const outer = useRef<HTMLDivElement>(null);
  const measure = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [widths, setWidths] = useState<TabWidths>({ full: [], compact: [], icon: [] });
  const density = usePrefs((s) => s.density);
  const sig = tabs.map((t) => `${t.key}:${t.label}:${typeof t.badge === 'string' || typeof t.badge === 'number' ? t.badge : t.badge ? 'b' : ''}:${t.dot ? 1 : 0}`).join('|');

  useLayoutEffect(() => {
    const el = outer.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth - parseFloat(getComputedStyle(el).paddingLeft) - parseFloat(getComputedStyle(el).paddingRight)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useLayoutEffect(() => {
    const m = measure.current;
    if (!m) return;
    const read = (mode: string) => Array.from(m.querySelectorAll<HTMLElement>(`[data-mode="${mode}"]`)).map((b) => Math.ceil(b.getBoundingClientRect().width));
    setWidths({ full: read('full'), compact: read('compact'), icon: read('icon') });
  }, [sig, density]);

  const active = tabs.findIndex((t) => t.key === value);
  const fit: TabFit = widths.full.length === tabs.length ? fitTabs(widths, width, active, MORE_W, GAP, prefer) : { mode: 'full', visible: tabs.map((_, i) => i), overflow: [] };

  const body = (t: TabDef<K>, mode: string) =>
    mode === 'icon' ? (
      <>
        {t.icon}
        {t.badge !== undefined && t.badge !== null && t.badge !== 0 && t.badge !== '' && <span className="badge">{t.badge}</span>}
        {t.dot && <span className="dot ok" />}
      </>
    ) : (
      <>
        {t.showIcon && t.icon}
        {t.label}
        {t.badge !== undefined && t.badge !== null && t.badge !== 0 && t.badge !== '' && <span className="badge">{t.badge}</span>}
        {t.dot && <span className="dot ok" />}
      </>
    );

  return (
    <div ref={outer} className={clsx('subtabs rtabs', `rtabs-${fit.mode}`, className)} role="tablist" aria-label={label}>
      {fit.visible.map((i) => {
        const t = tabs[i];
        return (
          <button key={t.key} className={clsx('subtab', fit.mode !== 'full' && fit.mode, t.key === value && 'on')} onClick={() => onChange(t.key)} role="tab" aria-selected={t.key === value} title={fit.mode === 'icon' ? (t.title ?? t.label) : t.title}>
            {body(t, fit.mode)}
          </button>
        );
      })}
      {fit.overflow.length > 0 && (
        <Menu
          align="right"
          items={fit.overflow.map((i) => ({ label: tabs[i].label, icon: tabs[i].icon, hint: tabs[i].badge ? <span className="badge">{tabs[i].badge}</span> : undefined, onSelect: () => onChange(tabs[i].key) }))}
          trigger={(p) => (
            <button className="subtab icon rtabs-more" {...p} title="More tabs" aria-label="More tabs">
              <Ellipsis />
            </button>
          )}
        />
      )}
      {/* Off-screen copies in every mode, measured to decide what fits. */}
      <div ref={measure} className="rtabs-measure" aria-hidden>
        {(['full', 'compact', 'icon'] as const).map((mode) =>
          tabs.map((t) => (
            <span key={`${mode}-${t.key}`} data-mode={mode} className={clsx('subtab', mode !== 'full' && mode)}>
              {body(t, mode)}
            </span>
          )),
        )}
      </div>
    </div>
  );
}
