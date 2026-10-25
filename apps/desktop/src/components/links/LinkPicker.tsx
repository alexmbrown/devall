import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { create } from 'zustand';
import clsx from 'clsx';
import { CornerDownLeft, ExternalLink, Search } from 'lucide-react';
import { filterLinkItems, type LinkItem } from '@/lib/links/fuzzy';
import { allLinkItems, linkProvider, useLinksVersion } from '@/lib/links/registry';
import type { LinkKind } from '@/lib/links/syntax';
import { kindIcon } from './kindIcons';
import './links.css';

export interface PickedLink {
  link: string;
  title: string;
  /** Null for a typed URL or file path (freeform). */
  kind: LinkKind | null;
}

export interface PickOptions {
  /** Placeholder text. */
  placeholder?: string;
  /** Show it here (a caret or button, viewport px); centred like the palette otherwise. */
  anchor?: { x: number; y: number };
  kinds?: LinkKind[];
  /** Also accept what's typed as a link (URLs, file paths) — the to-do "Add link…". */
  freeform?: boolean;
}

interface Request extends PickOptions {
  resolve(v: PickedLink | null): void;
}

const usePicker = create<{ req: Request | null }>(() => ({ req: null }));

/** Opens the "Link to…" picker; resolves with the picked item (null when dismissed). */
export function pickLink(opts: PickOptions = {}): Promise<PickedLink | null> {
  return new Promise((resolve) => {
    usePicker.getState().req?.resolve(null);
    usePicker.setState({ req: { ...opts, resolve } });
  });
}

/** Mounted once in the app shell. */
export function LinkPickerHost() {
  const req = usePicker((s) => s.req);
  if (!req) return null;
  const close = (v: PickedLink | null) => {
    usePicker.setState({ req: null });
    req.resolve(v);
  };
  return createPortal(<Picker req={req} close={close} />, document.body);
}

/** A URL, an absolute path or something path-like. */
const looksLikeLink = (s: string) => /^[a-z][a-z0-9+.-]*:/i.test(s) || /^([a-zA-Z]:)?[\\/]/.test(s) || /[\\/]|\.\w{1,8}$/.test(s);

type Row = { kind: 'item'; item: LinkItem } | { kind: 'raw'; value: string };

function Picker({ req, close }: { req: Request; close(v: PickedLink | null): void }) {
  const [q, setQ] = useState('');
  const [hl, setHl] = useState(0);
  const [pos, setPos] = useState<CSSProperties | undefined>(undefined);
  const box = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const v = useLinksVersion();
  // eslint-disable-next-line react-hooks/exhaustive-deps -- `v`: providers loaded more items
  const items = useMemo(() => allLinkItems(), [v]);
  const groups = useMemo(() => filterLinkItems(items, q, { kinds: req.kinds }), [items, q, req.kinds]);
  const typed = req.freeform ? q.trim() : '';
  const rawFirst = !!typed && looksLikeLink(typed);
  const rows: Row[] = useMemo(() => {
    const found: Row[] = groups.flatMap((g) => g.items.map((item) => ({ kind: 'item' as const, item })));
    if (!typed) return found;
    return rawFirst ? [{ kind: 'raw', value: typed }, ...found] : [...found, { kind: 'raw', value: typed }];
  }, [groups, typed, rawFirst]);

  useEffect(() => setHl(0), [q]);
  useEffect(() => {
    list.current?.querySelector('.item.hl')?.scrollIntoView({ block: 'nearest' });
  }, [hl]);
  // Next to the caret / button when anchored, kept on screen.
  useLayoutEffect(() => {
    const a = req.anchor;
    const el = box.current;
    if (!a || !el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.max(8, Math.min(a.x, window.innerWidth - w - 8));
    const below = a.y + 6;
    const top = below + h > window.innerHeight - 8 ? Math.max(8, a.y - h - 24) : below;
    setPos({ position: 'fixed', left, top, margin: 0 });
  }, [req.anchor, rows.length]);

  const pick = (r?: Row) => {
    if (!r) return;
    if (r.kind === 'raw') close({ link: r.value, title: r.value, kind: null });
    else close({ link: r.item.link, title: r.item.title, kind: r.item.kind });
  };

  let i = -1;
  const row = (r: Row, key: string, icon: ReactNode, body: ReactNode) => {
    const index = ++i;
    return (
      <div key={key} className={clsx('item', index === hl && 'hl')} onMouseEnter={() => setHl(index)} onMouseDown={(e) => e.preventDefault()} onClick={() => pick(r)}>
        {icon}
        {body}
        {index === hl && <CornerDownLeft className="dl-enter" />}
      </div>
    );
  };
  const rawRow = (value: string) => row({ kind: 'raw', value }, 'raw', <ExternalLink />, <span className="grow truncate">Link “{value}”</span>);

  return (
    <div className={clsx('overlay', req.anchor && 'dl-overlay-anchored')} onMouseDown={(e) => e.target === e.currentTarget && close(null)}>
      <div ref={box} className="modal palette dl-picker" style={req.anchor ? (pos ?? { position: 'fixed', left: -9999, top: -9999 }) : undefined} role="dialog" aria-label="Link to">
        <input
          autoFocus
          value={q}
          placeholder={req.placeholder ?? 'Link to… requests, notes, to-dos, environments, stubs, pages  (todo: narrows)'}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Escape') close(null);
            else if (e.key === 'ArrowDown' || (e.key === 'n' && e.ctrlKey)) {
              e.preventDefault();
              setHl((h) => Math.min(rows.length - 1, h + 1));
            } else if (e.key === 'ArrowUp' || (e.key === 'p' && e.ctrlKey)) {
              e.preventDefault();
              setHl((h) => Math.max(0, h - 1));
            } else if (e.key === 'Enter') {
              e.preventDefault();
              pick(rows[hl]);
            }
          }}
        />
        <div className="results" ref={list}>
          {rows.length === 0 && (
            <div className="empty">
              <Search />
              <p>{q.trim() ? 'Nothing matches' : 'Nothing to link to yet'}</p>
            </div>
          )}
          {rawFirst && rawRow(typed)}
          {groups.map((g) => (
            <div key={g.label}>
              <div className="group">{g.label}</div>
              {g.items.map((it) =>
                row(
                  { kind: 'item', item: it },
                  it.link,
                  <span className="dl-icon">{(it.icon as ReactNode) ?? linkProvider(it.kind)?.icon(it.kind) ?? kindIcon(it.kind)}</span>,
                  <>
                    <span className="truncate dl-pick-title">{it.title}</span>
                    {it.detail && <span className="subtle xs truncate grow mono">{it.detail}</span>}
                  </>,
                ),
              )}
            </div>
          ))}
          {typed && !rawFirst && (
            <>
              <div className="group">Other</div>
              {rawRow(typed)}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
