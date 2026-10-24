import { useState } from 'react';
import { Trash2, ListPlus, Text } from 'lucide-react';
import { Checkbox } from '@/components/ui/misc';
import { VarInput } from '@/components/VarInput';
import type { KeyValue, VarInfo } from '@/lib/types';

interface Props {
  rows: KeyValue[];
  onChange(rows: KeyValue[]): void;
  vars: VarInfo[];
  namePlaceholder?: string;
  valuePlaceholder?: string;
  /** Suggestions for the name column (e.g. common header names). */
  nameSuggestions?: string[];
}

const toText = (rows: KeyValue[]) => rows.map((r) => `${r.enabled === false ? '# ' : ''}${r.name}: ${r.value}`).join('\n');

function fromText(text: string): KeyValue[] {
  return text
    .split('\n')
    .map((l) => l.trimEnd())
    .filter((l) => l.trim())
    .map((l) => {
      const disabled = l.startsWith('#');
      const body = disabled ? l.replace(/^#\s?/, '') : l;
      const i = body.indexOf(':');
      const name = (i < 0 ? body : body.slice(0, i)).trim();
      const value = i < 0 ? '' : body.slice(i + 1).trim();
      return disabled ? { name, value, enabled: false } : { name, value };
    });
}

/** Editable name/value rows with enable toggles; always shows a blank row to type into. */
export function KeyValueEditor({ rows, onChange, vars, namePlaceholder = 'Name', valuePlaceholder = 'Value', nameSuggestions }: Props) {
  const [bulk, setBulk] = useState(false);
  const [bulkText, setBulkText] = useState('');
  const list = [...rows, { name: '', value: '' }];
  const listId = nameSuggestions ? `kv-suggest-${namePlaceholder}` : undefined;

  const update = (i: number, patch: Partial<KeyValue>) => {
    const next = list.map((r, j) => (j === i ? { ...r, ...patch } : r));
    // Drop the trailing blank row; keep enabled=true implicit.
    onChange(
      next
        .filter((r, j) => j < next.length - 1 || r.name || r.value)
        .map((r) => (r.enabled === true ? (({ enabled: _e, ...rest }) => rest)(r) : r)),
    );
  };

  if (bulk)
    return (
      <div className="col-sm">
        <textarea
          className="textarea mono"
          style={{ minHeight: 160 }}
          value={bulkText}
          onChange={(e) => {
            setBulkText(e.target.value);
            onChange(fromText(e.target.value));
          }}
          placeholder={'Name: value\n# Disabled: value'}
          spellCheck={false}
        />
        <div className="row">
          <button className="btn btn-sm btn-ghost" onClick={() => setBulk(false)}>
            <ListPlus /> Table view
          </button>
          <span className="subtle xs">One per line. Prefix with # to disable.</span>
        </div>
      </div>
    );

  return (
    <div className="kv-editor">
      {listId && (
        <datalist id={listId}>
          {nameSuggestions!.map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
      )}
      {list.map((r, i) => {
        const blank = i === list.length - 1;
        return (
          <div key={i} className={`kv-row${r.enabled === false ? ' off' : ''}`}>
            <span className="kv-check">{!blank && <Checkbox checked={r.enabled !== false} onChange={(v) => update(i, { enabled: v })} title="Enabled" />}</span>
            <input className="input sm mono" value={r.name} placeholder={namePlaceholder} list={listId} spellCheck={false} onChange={(e) => update(i, { name: e.target.value })} />
            <VarInput value={r.value} onChange={(v) => update(i, { value: v })} vars={vars} placeholder={valuePlaceholder} className="sm" />
            <span className="kv-actions">
              {!blank && (
                <button className="icon-btn sm danger" onClick={() => onChange(rows.filter((_, j) => j !== i))} title="Remove" aria-label="Remove row">
                  <Trash2 />
                </button>
              )}
            </span>
          </div>
        );
      })}
      <div className="row mt-2">
        <button
          className="btn btn-sm btn-ghost"
          onClick={() => {
            setBulkText(toText(rows));
            setBulk(true);
          }}
        >
          <Text /> Bulk edit
        </button>
      </div>
    </div>
  );
}

export const COMMON_HEADERS = [
  'Accept',
  'Accept-Encoding',
  'Accept-Language',
  'Authorization',
  'Cache-Control',
  'Content-Type',
  'Cookie',
  'If-Match',
  'If-None-Match',
  'Origin',
  'Prefer',
  'Referer',
  'User-Agent',
  'X-API-Key',
  'X-Correlation-ID',
  'X-Request-ID',
];
