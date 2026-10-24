import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { ArrowDown, ArrowUp, Braces, Copy, Download, Filter, FilterX, PanelRight, Plus, Trash2, X } from 'lucide-react';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { CopyButton, SearchInput, copyText } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { ColumnMeta, Dialect } from '@/lib/api/db';
import { cellPreview, cellText, compareValues, fmtCount, toCsv, toInsert, toJson, toMarkdown, toTsv, tryPrettyJson } from './format';

const ROW_H = 24;
const GUTTER_W = 56;
const OVERSCAN = 12;

/** Pending grid edits, by index into `rows` (inserts are appended rows). */
export interface PendingEdits {
  updates: Record<number, Record<number, unknown>>;
  deletes: Record<number, true>;
  inserts: unknown[][];
}

export const emptyEdits = (): PendingEdits => ({ updates: {}, deletes: {}, inserts: [] });

export const editCount = (p: PendingEdits) => Object.keys(p.updates).length + Object.keys(p.deletes).length + p.inserts.length;

export interface GridEditing {
  /** False shows why editing is off (read-only, no primary key, …). */
  enabled: boolean;
  reason?: string;
  pending: PendingEdits;
  onChange(p: PendingEdits): void;
}

interface Props {
  columns: ColumnMeta[];
  rows: unknown[][];
  dialect?: Dialect;
  /** Qualified table name for "Copy as INSERT". */
  tableName?: string;
  editing?: GridEditing;
  /** Extra toolbar content (left). */
  toolbar?: ReactNode;
  /** Status text (right of the toolbar). */
  status?: ReactNode;
  more?: boolean;
  onLoadMore?(): void;
  loadingMore?: boolean;
}

type Sort = { col: number; dir: 1 | -1 } | null;

function initialWidths(columns: ColumnMeta[], rows: unknown[][]): number[] {
  return columns.map((c, i) => {
    let len = c.name.length + 2;
    for (let r = 0; r < Math.min(rows.length, 60); r++) len = Math.max(len, cellPreview(rows[r][i], 60).length);
    return Math.max(64, Math.min(360, len * 7.2 + 20));
  });
}

function matches(v: unknown, f: string): boolean {
  if (!f) return true;
  if (f === 'NULL') return v === null || v === undefined;
  if (f === '!NULL') return !(v === null || v === undefined);
  const neg = f.startsWith('!');
  const q = (neg ? f.slice(1) : f).toLowerCase();
  const hit = cellText(v).toLowerCase().includes(q);
  return neg ? !hit : hit;
}

/** Parses typed text for a column of `kind`. */
function parseInput(text: string, kind: ColumnMeta['kind']): unknown {
  if ((kind === 'int' || kind === 'float') && /^-?\d+(\.\d+)?([eE][-+]?\d+)?$/.test(text.trim())) {
    const n = Number(text);
    if (Number.isSafeInteger(n) || kind === 'float') return n;
    return text.trim();
  }
  if (kind === 'bool') {
    if (/^(true|t|1|yes)$/i.test(text.trim())) return true;
    if (/^(false|f|0|no)$/i.test(text.trim())) return false;
  }
  if (kind === 'json') {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }
  return text;
}

/** Virtualized result grid: only visible rows are rendered, so 100k+ rows stay smooth. */
export function ResultsGrid({ columns, rows, dialect, tableName, editing, toolbar, status, more, onLoadMore, loadingMore }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(400);
  const [widths, setWidths] = useState<number[]>(() => initialWidths(columns, rows));
  const [sort, setSort] = useState<Sort>(null);
  const [quick, setQuick] = useState('');
  const [showFilters, setShowFilters] = useState(false);
  const [filters, setFilters] = useState<string[]>(() => columns.map(() => ''));
  const [cursor, setCursor] = useState<{ row: number; col: number } | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
  const [detail, setDetail] = useState(false);
  const [editingCell, setEditingCell] = useState<{ row: number; col: number; text: string } | null>(null);
  const ctx = useContextMenu();

  // New result → reset view state.
  useEffect(() => {
    setWidths(initialWidths(columns, rows));
    setFilters(columns.map(() => ''));
    setSort(null);
    setSelected(new Set());
    setCursor(null);
    setEditingCell(null);
    scrollRef.current?.scrollTo({ top: 0 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [columns]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    setViewH(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  const pending = editing?.pending;
  /** All rows including pending inserts; values with pending updates applied. */
  const allRows = useMemo(() => {
    if (!pending || (!Object.keys(pending.updates).length && !pending.inserts.length)) return rows;
    const out = rows.map((r, i) => {
      const u = pending.updates[i];
      if (!u) return r;
      const copy = r.slice();
      for (const [c, v] of Object.entries(u)) copy[Number(c)] = v;
      return copy;
    });
    return out.concat(pending.inserts);
  }, [rows, pending]);

  /** View order: indices into allRows after filtering and sorting. */
  const order = useMemo(() => {
    const q = quick.trim().toLowerCase();
    const active = filters.some((f) => f.trim());
    let idx: number[] = [];
    for (let i = 0; i < allRows.length; i++) {
      const r = allRows[i];
      if (q && !r.some((v) => cellText(v).toLowerCase().includes(q))) continue;
      if (active && !filters.every((f, c) => matches(r[c], f.trim()))) continue;
      idx.push(i);
    }
    if (sort) {
      const { col, dir } = sort;
      idx = idx.slice().sort((a, b) => dir * compareValues(allRows[a][col], allRows[b][col]) || a - b);
    }
    return idx;
  }, [allRows, quick, filters, sort]);

  const totalW = GUTTER_W + widths.reduce((a, b) => a + b, 0);
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(order.length, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (el) setScrollTop(el.scrollTop);
  }, []);

  const scrollToRow = (viewIdx: number) => {
    const el = scrollRef.current;
    if (!el) return;
    const top = viewIdx * ROW_H;
    const headerH = ROW_H * (showFilters ? 2 : 1);
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + el.clientHeight - headerH) el.scrollTop = top + ROW_H - el.clientHeight + headerH;
  };

  const selectedRows = (): unknown[][] => {
    if (selected.size) return order.filter((i) => selected.has(i)).map((i) => allRows[i]);
    return order.map((i) => allRows[i]);
  };

  const copyAs = (fmt: 'csv' | 'tsv' | 'json' | 'md' | 'insert') => {
    const rs = selectedRows();
    const text =
      fmt === 'csv'
        ? toCsv(columns, rs)
        : fmt === 'tsv'
          ? toTsv(columns, rs)
          : fmt === 'json'
            ? toJson(columns, rs)
            : fmt === 'md'
              ? toMarkdown(columns, rs)
              : toInsert(tableName ?? 'my_table', columns, rs, dialect ?? 'postgres');
    void copyText(text, `Copied ${fmtCount(rs.length)} row${rs.length === 1 ? '' : 's'}`);
  };

  const saveAs = async (fmt: 'csv' | 'json') => {
    const rs = selectedRows();
    const dest = await saveDialog({ defaultPath: `${tableName?.replace(/["`]/g, '') ?? 'result'}.${fmt}`, filters: [{ name: fmt.toUpperCase(), extensions: [fmt] }] });
    if (!dest) return;
    try {
      await api.writeTextFile(dest, fmt === 'csv' ? toCsv(columns, rs) : toJson(columns, rs));
      toast.success(`Saved ${fmtCount(rs.length)} rows`, dest);
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  // ---------- editing ----------
  const canEdit = !!editing?.enabled;
  const isInserted = (dataIdx: number) => dataIdx >= rows.length;

  const commitCell = (dataIdx: number, col: number, value: unknown) => {
    if (!editing) return;
    const p = editing.pending;
    if (isInserted(dataIdx)) {
      const inserts = p.inserts.map((r, i) => (i === dataIdx - rows.length ? r.map((v, c) => (c === col ? value : v)) : r));
      editing.onChange({ ...p, inserts });
      return;
    }
    const original = rows[dataIdx][col];
    const updates = { ...p.updates };
    const rowUpd = { ...(updates[dataIdx] ?? {}) };
    if (JSON.stringify(original) === JSON.stringify(value)) delete rowUpd[col];
    else rowUpd[col] = value;
    if (Object.keys(rowUpd).length) updates[dataIdx] = rowUpd;
    else delete updates[dataIdx];
    editing.onChange({ ...p, updates });
  };

  const toggleDelete = (dataIdxs: number[]) => {
    if (!editing) return;
    const p = editing.pending;
    const deletes = { ...p.deletes };
    let inserts = p.inserts;
    const removeInserted = dataIdxs.filter(isInserted).map((i) => i - rows.length);
    if (removeInserted.length) inserts = inserts.filter((_, i) => !removeInserted.includes(i));
    for (const i of dataIdxs.filter((i) => !isInserted(i))) {
      if (deletes[i]) delete deletes[i];
      else deletes[i] = true;
    }
    editing.onChange({ ...p, deletes, inserts });
    setSelected(new Set());
  };

  const addRow = () => {
    if (!editing) return;
    const p = editing.pending;
    editing.onChange({ ...p, inserts: [...p.inserts, columns.map(() => null)] });
    requestAnimationFrame(() => {
      const el = scrollRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  };

  const startEdit = (viewIdx: number, col: number) => {
    if (!canEdit) return;
    const dataIdx = order[viewIdx];
    if (dataIdx === undefined || editing?.pending.deletes[dataIdx]) return;
    const v = allRows[dataIdx][col];
    setEditingCell({ row: viewIdx, col, text: v === null || v === undefined ? '' : typeof v === 'object' ? JSON.stringify(v) : String(v) });
  };

  const finishEdit = (commit: boolean, setNull = false) => {
    if (!editingCell) return;
    if (commit) {
      const dataIdx = order[editingCell.row];
      const value = setNull ? null : parseInput(editingCell.text, columns[editingCell.col].kind);
      commitCell(dataIdx, editingCell.col, value);
    }
    setEditingCell(null);
    scrollRef.current?.focus();
  };

  // ---------- keyboard ----------
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (editingCell) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      setSelected(new Set(order));
      return;
    }
    if (mod && e.key.toLowerCase() === 'c') {
      e.preventDefault();
      if (selected.size) copyAs('tsv');
      else if (cursor) {
        const v = allRows[order[cursor.row]]?.[cursor.col];
        void copyText(v === null || v === undefined ? 'NULL' : cellText(v), 'Copied value');
      }
      return;
    }
    if (!cursor) {
      if (['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(e.key) && order.length) {
        e.preventDefault();
        setCursor({ row: 0, col: 0 });
      }
      return;
    }
    const page = Math.max(1, Math.floor(viewH / ROW_H) - 2);
    let { row, col } = cursor;
    switch (e.key) {
      case 'ArrowDown':
        row = Math.min(order.length - 1, row + 1);
        break;
      case 'ArrowUp':
        row = Math.max(0, row - 1);
        break;
      case 'ArrowLeft':
        col = Math.max(0, col - 1);
        break;
      case 'ArrowRight':
        col = Math.min(columns.length - 1, col + 1);
        break;
      case 'PageDown':
        row = Math.min(order.length - 1, row + page);
        break;
      case 'PageUp':
        row = Math.max(0, row - page);
        break;
      case 'Home':
        if (mod) row = 0;
        col = 0;
        break;
      case 'End':
        if (mod) row = order.length - 1;
        col = columns.length - 1;
        break;
      case 'Enter':
      case 'F2':
        e.preventDefault();
        if (canEdit && (e.key === 'F2' || !detail)) startEdit(row, col);
        else setDetail((d) => !d);
        return;
      case 'Delete':
        if (canEdit) {
          e.preventDefault();
          toggleDelete(selected.size ? [...selected] : [order[row]]);
        }
        return;
      case ' ':
        e.preventDefault();
        setSelected((s) => {
          const n = new Set(s);
          const d = order[row];
          if (n.has(d)) n.delete(d);
          else n.add(d);
          return n;
        });
        return;
      case 'Escape':
        setSelected(new Set());
        return;
      default:
        return;
    }
    e.preventDefault();
    setCursor({ row, col });
    scrollToRow(row);
  };

  const clickGutter = (e: React.MouseEvent, viewIdx: number) => {
    const d = order[viewIdx];
    setCursor({ row: viewIdx, col: cursor?.col ?? 0 });
    if (e.shiftKey && anchor !== null) {
      const a = order.indexOf(anchor);
      const [lo, hi] = a < viewIdx ? [a, viewIdx] : [viewIdx, a];
      setSelected(new Set(order.slice(lo, hi + 1)));
      return;
    }
    setAnchor(d);
    setSelected((s) => {
      if (e.ctrlKey || e.metaKey) {
        const n = new Set(s);
        if (n.has(d)) n.delete(d);
        else n.add(d);
        return n;
      }
      return s.size === 1 && s.has(d) ? new Set() : new Set([d]);
    });
  };

  const cellMenu = (e: React.MouseEvent, viewIdx: number, col: number) => {
    const d = order[viewIdx];
    const v = allRows[d][col];
    setCursor({ row: viewIdx, col });
    const items: MenuItemDef[] = [
      { label: 'Copy value', icon: <Copy />, onSelect: () => void copyText(v === null || v === undefined ? 'NULL' : cellText(v), 'Copied value') },
      { label: 'Copy row as JSON', icon: <Braces />, onSelect: () => void copyText(JSON.stringify(Object.fromEntries(columns.map((c, i) => [c.name, allRows[d][i]])), null, 2), 'Copied row') },
      { label: 'Filter by this value', icon: <Filter />, onSelect: () => {
        setShowFilters(true);
        setFilters((f) => f.map((x, i) => (i === col ? (v === null || v === undefined ? 'NULL' : cellText(v)) : x)));
      } },
      { label: 'Row details', icon: <PanelRight />, hint: 'Enter', onSelect: () => setDetail(true) },
    ];
    if (canEdit) {
      items.push(
        { separator: true },
        { label: 'Edit cell', hint: 'F2', onSelect: () => startEdit(viewIdx, col) },
        { label: 'Set NULL', onSelect: () => commitCell(d, col, null) },
        { label: editing?.pending.deletes[d] ? 'Undo delete' : 'Delete row', icon: <Trash2 />, danger: !editing?.pending.deletes[d], hint: 'Del', onSelect: () => toggleDelete([d]) },
      );
    }
    ctx.open(e, items);
  };

  // ---------- column resize ----------
  const startResize = (e: React.PointerEvent, col: number) => {
    e.preventDefault();
    e.stopPropagation();
    const x0 = e.clientX;
    const w0 = widths[col];
    const move = (ev: PointerEvent) => setWidths((w) => w.map((x, i) => (i === col ? Math.max(40, w0 + ev.clientX - x0) : x)));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    document.body.style.cursor = 'col-resize';
  };

  const cycleSort = (col: number) =>
    setSort((s) => (!s || s.col !== col ? { col, dir: 1 } : s.dir === 1 ? { col, dir: -1 } : null));

  const detailRow = cursor ? allRows[order[cursor.row]] : undefined;
  const filtersActive = quick.trim() !== '' || filters.some((f) => f.trim());
  const headerRows = showFilters ? 2 : 1;

  return (
    <div className="db-grid-wrap">
      <div className="toolbar db-grid-toolbar">
        {toolbar}
        <SearchInput value={quick} onChange={setQuick} placeholder="Filter rows…" className="db-grid-search" />
        <button className={clsx('icon-btn sm', showFilters && 'active')} onClick={() => setShowFilters((v) => !v)} title="Column filters (text, !text, NULL, !NULL)" aria-label="Column filters">
          <Filter />
        </button>
        {filtersActive && (
          <button
            className="icon-btn sm"
            onClick={() => {
              setQuick('');
              setFilters(columns.map(() => ''));
            }}
            title="Clear filters"
            aria-label="Clear filters"
          >
            <FilterX />
          </button>
        )}
        {canEdit && (
          <button className="btn btn-sm btn-ghost" onClick={addRow} title="Add a row">
            <Plus /> Row
          </button>
        )}
        {canEdit && selected.size > 0 && (
          <button className="btn btn-sm btn-ghost" onClick={() => toggleDelete([...selected])} title="Delete selected rows (Del)">
            <Trash2 /> Delete {selected.size}
          </button>
        )}
        {editing && !editing.enabled && editing.reason && <span className="subtle small truncate" title={editing.reason}>{editing.reason}</span>}
        <span className="grow" />
        <span className="subtle small nowrap">
          {status}
          {filtersActive && ` · ${fmtCount(order.length)} shown`}
          {selected.size > 0 && ` · ${fmtCount(selected.size)} selected`}
        </span>
        <Menu
          align="right"
          trigger={(p) => (
            <button className="btn btn-sm" {...p} title={selected.size ? 'Copy / export the selected rows' : 'Copy / export all (filtered) rows'}>
              <Download /> Export
            </button>
          )}
          items={[
            { heading: selected.size ? `${fmtCount(selected.size)} selected rows` : `${fmtCount(order.length)} rows` },
            { label: 'Copy as CSV', onSelect: () => copyAs('csv') },
            { label: 'Copy as TSV (spreadsheets)', onSelect: () => copyAs('tsv') },
            { label: 'Copy as JSON', onSelect: () => copyAs('json') },
            { label: 'Copy as Markdown', onSelect: () => copyAs('md') },
            { label: 'Copy as SQL INSERT', onSelect: () => copyAs('insert') },
            { separator: true },
            { label: 'Save as CSV…', onSelect: () => void saveAs('csv') },
            { label: 'Save as JSON…', onSelect: () => void saveAs('json') },
          ]}
        />
        <button className={clsx('icon-btn sm', detail && 'active')} onClick={() => setDetail((d) => !d)} title="Row details (Enter)" aria-label="Row details">
          <PanelRight />
        </button>
      </div>
      <div className="db-grid-body">
        <div
          ref={scrollRef}
          className="db-grid"
          tabIndex={0}
          onScroll={onScroll}
          onKeyDown={onKeyDown}
          role="grid"
          aria-rowcount={order.length}
          aria-colcount={columns.length}
        >
          <div style={{ width: totalW, height: (order.length + headerRows) * ROW_H, position: 'relative' }}>
            <div className="db-grid-head" style={{ width: totalW }}>
              <div className="db-grid-row db-grid-header" style={{ height: ROW_H }}>
                <div className="db-cell db-gutter" style={{ width: GUTTER_W }} title="Rows">
                  #
                </div>
                {columns.map((c, i) => (
                  <div
                    key={i}
                    className={clsx('db-cell db-hcell', sort?.col === i && 'sorted')}
                    style={{ width: widths[i] }}
                    onClick={() => cycleSort(i)}
                    title={`${c.name}${c.typeName ? ` · ${c.typeName}` : ''} — click to sort`}
                  >
                    <span className="truncate grow">{c.name}</span>
                    {c.typeName && <span className="db-type">{c.typeName}</span>}
                    {sort?.col === i && (sort.dir === 1 ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
                    <span className="db-resize" onPointerDown={(e) => startResize(e, i)} onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => {
                      e.stopPropagation();
                      setWidths((w) => w.map((x, j) => (j === i ? initialWidths([c], allRows.map((r) => [r[i]]))[0] : x)));
                    }} />
                  </div>
                ))}
              </div>
              {showFilters && (
                <div className="db-grid-row db-grid-filters" style={{ height: ROW_H }}>
                  <div className="db-cell db-gutter" style={{ width: GUTTER_W }}>
                    <Filter size={11} />
                  </div>
                  {columns.map((c, i) => (
                    <div key={i} className="db-cell" style={{ width: widths[i], padding: 1 }}>
                      <input
                        className="db-filter-input"
                        value={filters[i] ?? ''}
                        placeholder={`${c.name}…`}
                        onChange={(e) => setFilters((f) => f.map((x, j) => (j === i ? e.target.value : x)))}
                        onKeyDown={(e) => e.stopPropagation()}
                      />
                    </div>
                  ))}
                </div>
              )}
            </div>
            {order.slice(first, last).map((dataIdx, k) => {
              const viewIdx = first + k;
              const r = allRows[dataIdx];
              const upd = pending?.updates[dataIdx];
              const deleted = !!pending?.deletes[dataIdx];
              const inserted = isInserted(dataIdx);
              return (
                <GridRow
                  key={dataIdx}
                  top={(viewIdx + headerRows) * ROW_H}
                  row={r}
                  widths={widths}
                  columns={columns}
                  viewIdx={viewIdx}
                  dataIdx={dataIdx}
                  selected={selected.has(dataIdx)}
                  cursorCol={cursor?.row === viewIdx ? cursor.col : -1}
                  updated={upd}
                  deleted={deleted}
                  inserted={inserted}
                  editCol={editingCell?.row === viewIdx ? editingCell.col : -1}
                  editText={editingCell?.row === viewIdx ? editingCell.text : ''}
                  onEditText={(t) => setEditingCell((c) => (c ? { ...c, text: t } : c))}
                  onFinishEdit={finishEdit}
                  onGutter={clickGutter}
                  onCell={(col) => setCursor({ row: viewIdx, col })}
                  onCellDouble={(col) => (canEdit ? startEdit(viewIdx, col) : setDetail(true))}
                  onCellMenu={cellMenu}
                />
              );
            })}
          </div>
          {order.length === 0 && <div className="db-grid-empty">{rows.length ? 'No rows match the filters' : 'No rows'}</div>}
        </div>
        {detail && (
          <div className="db-detail">
            <div className="toolbar">
              <span className="strong small grow">Row {cursor ? cursor.row + 1 : '—'}</span>
              {detailRow && <CopyButton text={() => JSON.stringify(Object.fromEntries(columns.map((c, i) => [c.name, detailRow[i]])), null, 2)} title="Copy row as JSON" />}
              <button className="icon-btn sm" onClick={() => setDetail(false)} aria-label="Close row details">
                <X />
              </button>
            </div>
            <div className="db-detail-body">
              {detailRow ? (
                columns.map((c, i) => {
                  const v = detailRow[i];
                  const text = v === null || v === undefined ? null : typeof v === 'object' ? JSON.stringify(v, null, 2) : (tryPrettyJson(String(v)) ?? String(v));
                  return (
                    <div key={i} className={clsx('db-detail-item', cursor?.col === i && 'on')} onClick={() => cursor && setCursor({ ...cursor, col: i })}>
                      <div className="db-detail-name">
                        <span className="truncate">{c.name}</span>
                        <span className="db-type">{c.typeName}</span>
                      </div>
                      {text === null ? <div className="db-null">NULL</div> : <pre className="db-detail-value">{text}</pre>}
                    </div>
                  );
                })
              ) : (
                <div className="subtle small p-4">Select a cell to see its row.</div>
              )}
            </div>
          </div>
        )}
      </div>
      {more && onLoadMore && (
        <div className="db-grid-more">
          <button className="btn btn-sm" onClick={onLoadMore} disabled={loadingMore}>
            {loadingMore ? 'Loading…' : 'Load more rows'}
          </button>
        </div>
      )}
      {ctx.node}
    </div>
  );
}

interface RowProps {
  top: number;
  row: unknown[];
  widths: number[];
  columns: ColumnMeta[];
  viewIdx: number;
  dataIdx: number;
  selected: boolean;
  cursorCol: number;
  updated?: Record<number, unknown>;
  deleted: boolean;
  inserted: boolean;
  editCol: number;
  editText: string;
  onEditText(t: string): void;
  onFinishEdit(commit: boolean, setNull?: boolean): void;
  onGutter(e: React.MouseEvent, viewIdx: number): void;
  onCell(col: number): void;
  onCellDouble(col: number): void;
  onCellMenu(e: React.MouseEvent, viewIdx: number, col: number): void;
}

const GridRow = memo(function GridRow(p: RowProps) {
  return (
    <div className={clsx('db-grid-row', p.selected && 'selected', p.deleted && 'deleted', p.inserted && 'inserted')} style={{ top: p.top, height: ROW_H }} role="row">
      <div className="db-cell db-gutter" style={{ width: GUTTER_W }} onMouseDown={(e) => p.onGutter(e, p.viewIdx)}>
        {p.inserted ? '+' : p.viewIdx + 1}
      </div>
      {p.columns.map((c, i) => {
        const v = p.row[i];
        const isNull = v === null || v === undefined;
        if (p.editCol === i)
          return (
            <div key={i} className="db-cell editing" style={{ width: p.widths[i] }}>
              <input
                className="db-cell-input"
                autoFocus
                value={p.editText}
                onChange={(e) => p.onEditText(e.target.value)}
                onBlur={() => p.onFinishEdit(true)}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === 'Enter') p.onFinishEdit(true);
                  else if (e.key === 'Escape') p.onFinishEdit(false);
                  else if (e.key === 'Delete' && (e.ctrlKey || e.metaKey)) p.onFinishEdit(true, true);
                }}
                title="Enter saves · Esc cancels · Ctrl+Del sets NULL"
              />
            </div>
          );
        return (
          <div
            key={i}
            className={clsx('db-cell', p.cursorCol === i && 'cursor', c.kind && `k-${c.kind}`, typeof v === 'number' && 'num', p.updated && i in p.updated && 'changed')}
            style={{ width: p.widths[i] }}
            onMouseDown={() => p.onCell(i)}
            onDoubleClick={() => p.onCellDouble(i)}
            onContextMenu={(e) => p.onCellMenu(e, p.viewIdx, i)}
            role="gridcell"
          >
            {isNull ? <span className="db-null">NULL</span> : typeof v === 'boolean' ? <span className="db-bool">{String(v)}</span> : cellPreview(v)}
          </div>
        );
      })}
    </div>
  );
});
