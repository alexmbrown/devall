import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';
import { Resizer } from '@/components/ui/misc';
import { SIDE_PANE, usePrefs } from '@/store/prefs';

/** Below this view width the pane starts collapsed (splits make views narrow). */
export const SIDE_PANE_AUTO_COLLAPSE = 760;

interface SidePaneProps {
  /** Header label (e.g. "Connections"). */
  title?: ReactNode;
  /** Icon buttons at the right of the header (new, import, …). */
  actions?: ReactNode;
  /** Optional row under the header, e.g. a `SearchInput`. */
  toolbar?: ReactNode;
  /** Pinned to the bottom of the pane. */
  footer?: ReactNode;
  /** Accessible name (defaults to `title` when it is a string). */
  label?: string;
  className?: string;
  children: ReactNode;
}

/**
 * The left list/tree pane of a page (requests tree, notes, connections, schemas…). Every page uses
 * this one component so the pane has the same width everywhere (one persisted, resizable width),
 * the same header and the same empty-state layout. Render it as the first child of a `.split`;
 * it adds its own resize handle.
 *
 * It can collapse to a slim rail (header button; remembered for every page), and starts collapsed
 * when its view is narrower than {@link SIDE_PANE_AUTO_COLLAPSE} — a click on the rail opens it
 * again for that view.
 */
export function SidePane({ title, actions, toolbar, footer, label, className, children }: SidePaneProps) {
  const width = usePrefs((s) => s.sidePaneWidth);
  const collapsedPref = usePrefs((s) => s.sidePaneCollapsed);
  const setPref = usePrefs((s) => s.set);
  const ref = useRef<HTMLElement>(null);
  const [narrow, setNarrow] = useState(false);
  // An explicit open/close in this view wins over the automatic narrow-view collapse.
  const [override, setOverride] = useState<boolean | null>(null);

  useLayoutEffect(() => {
    const host = ref.current?.parentElement;
    if (!host) return;
    const ro = new ResizeObserver(() => setNarrow(host.clientWidth < SIDE_PANE_AUTO_COLLAPSE));
    ro.observe(host);
    return () => ro.disconnect();
  }, []);

  const collapsed = override ?? (collapsedPref || narrow);
  const name = label ?? (typeof title === 'string' ? title : undefined);
  const setCollapsed = (v: boolean) => {
    setOverride(v);
    // Remember the choice for every page unless it only fixes a narrow split.
    if (!narrow) setPref('sidePaneCollapsed', v);
  };

  if (collapsed)
    return (
      <aside ref={ref} className={clsx('pane side side-pane collapsed', className)} aria-label={name}>
        <button className="icon-btn" onClick={() => setCollapsed(false)} title={name ? `Show ${name}` : 'Show side pane'} aria-label={name ? `Show ${name}` : 'Show side pane'}>
          <PanelLeftOpen />
        </button>
      </aside>
    );

  return (
    <>
      <aside ref={ref} className={clsx('pane side side-pane', className)} style={{ width }} aria-label={name}>
        {(title || actions) && (
          <div className="side-pane-head">
            <span className="side-pane-title truncate">{title}</span>
            {actions && <div className="side-pane-actions">{actions}</div>}
            <button className="icon-btn sm side-pane-collapse" onClick={() => setCollapsed(true)} title="Hide side pane" aria-label="Hide side pane">
              <PanelLeftClose />
            </button>
          </div>
        )}
        {toolbar && <div className="side-pane-tools">{toolbar}</div>}
        <div className="side-pane-body">{children}</div>
        {footer && <div className="side-pane-foot">{footer}</div>}
      </aside>
      <Resizer width={width} onChange={(w) => setPref('sidePaneWidth', w)} min={SIDE_PANE.min} max={SIDE_PANE.max} defaultValue={SIDE_PANE.default} />
    </>
  );
}
