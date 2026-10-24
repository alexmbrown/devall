import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { Columns2, PanelsTopLeft, Rows2 } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import { Resizer } from '@/components/ui/misc';
import { usePrefs, type ReqLayout } from '@/store/prefs';

/** Below this editor width, "automatic" stacks the response under the request. */
export const STACK_BELOW = 820;
/** The response keeps at least this much room when side by side. */
const MIN_RESPONSE_W = 320;

/** Width of an element, tracked with a ResizeObserver (0 until laid out). */
export function useElementWidth<T extends HTMLElement>(): [React.RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(el.clientWidth);
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

/** Whether the request and response stack, for a layout preference and the editor's width. */
export function isStacked(layout: ReqLayout, width: number): boolean {
  return layout === 'rows' || (layout === 'auto' && width > 0 && width < STACK_BELOW);
}

/**
 * Request pane + response pane: side by side when the editor is wide, stacked (response below,
 * resizable) when it is narrow — measured on the view, not the window, since splits make views
 * narrow. The arrangement can be forced with {@link LayoutToggle} (remembered in prefs).
 */
export function ReqResSplit({ request, response }: { request: ReactNode; response: ReactNode }) {
  const [ref, width] = useElementWidth<HTMLDivElement>();
  const layout = usePrefs((s) => s.reqLayout);
  const paneW = usePrefs((s) => s.requestPaneWidth);
  const paneH = usePrefs((s) => s.requestPaneHeight);
  const setPref = usePrefs((s) => s.set);
  const stacked = isStacked(layout, width);
  const w = width ? Math.max(280, Math.min(paneW, width - MIN_RESPONSE_W)) : paneW;
  return (
    <div ref={ref} className={clsx('split reqres', stacked && 'stacked')}>
      <div className="pane request-pane" style={stacked ? { height: paneH } : { width: w }}>
        {request}
      </div>
      {stacked ? (
        <Resizer axis="y" width={paneH} onChange={(h) => setPref('requestPaneHeight', h)} min={160} max={900} defaultValue={320} />
      ) : (
        <Resizer width={w} onChange={(x) => setPref('requestPaneWidth', x)} min={280} max={Math.max(300, width - MIN_RESPONSE_W)} defaultValue={560} />
      )}
      <div className="pane fill response-pane">{response}</div>
    </div>
  );
}

const LAYOUTS: { value: ReqLayout; label: string; icon: ReactNode }[] = [
  { value: 'auto', label: 'Automatic (by width)', icon: <PanelsTopLeft /> },
  { value: 'columns', label: 'Response on the right', icon: <Columns2 /> },
  { value: 'rows', label: 'Response below', icon: <Rows2 /> },
];

/** Header button choosing how request and response are arranged. */
export function LayoutToggle() {
  const layout = usePrefs((s) => s.reqLayout);
  const setPref = usePrefs((s) => s.set);
  const cur = LAYOUTS.find((l) => l.value === layout) ?? LAYOUTS[0];
  return (
    <Menu
      align="right"
      items={[{ heading: 'Response position' }, ...LAYOUTS.map((l) => ({ label: l.label, icon: l.icon, hint: l.value === layout ? '✓' : undefined, onSelect: () => setPref('reqLayout', l.value) }))]}
      trigger={(p) => (
        <button className="icon-btn" {...p} title={`Layout: ${cur.label}`} aria-label="Request and response layout">
          {cur.icon}
        </button>
      )}
    />
  );
}
