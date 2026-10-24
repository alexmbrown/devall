/**
 * "HTTP request → mock → randomize data": helpers for stub response bodies.
 * - FakeDataButton: pick a `{{random 'Category.key'}}` (or built-in random) helper to insert.
 * - RandomizeJsonButton: turn a JSON body into a response template with the same structure whose
 *   leaf values are random-data helpers (devall-data's WireMock template bridge).
 * - AiMockDataButton: generate realistic data shaped like the current body with the local AI.
 */
import { useMemo, useState } from 'react';
import { Dices, Shuffle } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Checkbox, SearchInput, Spinner, Callout } from '@/components/ui/misc';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { dataApi } from '@/lib/api/data';
import { errorMessage, isTauri } from '@/lib/ipc';
import { AiAssistButton } from '@/features/ai';
import { FAKER_KEYS, RANDOM_HELPERS, fakerGroups, fakerHelper } from '@/features/mocks/lib/faker';

export function FakeDataButton({ onInsert }: { onInsert(snippet: string): void }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState('');
  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return fakerGroups()
      .map((g) => ({ ...g, keys: g.keys.filter((k) => !needle || k.toLowerCase().includes(needle)) }))
      .filter((g) => g.keys.length);
  }, [q]);
  const builtins = RANDOM_HELPERS.filter((h) => !q || h.label.toLowerCase().includes(q.toLowerCase()) || h.snippet.toLowerCase().includes(q.toLowerCase()));
  const pick = (snippet: string) => {
    onInsert(snippet);
    setOpen(false);
  };
  return (
    <>
      <button className="btn btn-sm btn-ghost" onClick={() => setOpen(true)} title="Insert a random-data helper (turns response templating on)">
        <Dices /> Fake data
      </button>
      {open && (
        <Modal open onClose={() => setOpen(false)} title="Insert fake data" size="lg">
          <div className="col">
            <SearchInput value={q} onChange={setQ} placeholder={`Filter ${FAKER_KEYS.length} faker keys and helpers`} autoFocus />
            <div className="subtle xs">
              Inserted at the end of the body; response templating is switched on. <code className="code-inline">{"{{random '…'}}"}</code> is rendered by the built-in engine
              (stock WireMock needs its faker extension); the other helpers are WireMock built-ins.
            </div>
            <div className="mocks-faker">
              {builtins.length > 0 && (
                <div className="mocks-faker-group">
                  <span className="label">Built-in</span>
                  <div className="row-sm wrap">
                    {builtins.map((h) => (
                      <button key={h.snippet} className="tag mocks-chip" onClick={() => pick(h.snippet)} title={h.snippet}>
                        {h.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}
              {groups.map((g) => (
                <div key={g.group} className="mocks-faker-group">
                  <span className="label">{g.group}</span>
                  <div className="row-sm wrap">
                    {g.keys.map((k) => (
                      <button key={k} className="tag mocks-chip" onClick={() => pick(fakerHelper(k))} title={fakerHelper(k)}>
                        {k.split('.')[1]}
                      </button>
                    ))}
                  </div>
                </div>
              ))}
              {!groups.length && !builtins.length && <div className="subtle small">Nothing matches “{q}”.</div>}
            </div>
          </div>
        </Modal>
      )}
    </>
  );
}

export function RandomizeJsonButton({ json, onApply, disabled }: { json: unknown; onApply(template: string): void; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const [keepIds, setKeepIds] = useState(true);
  const [template, setTemplate] = useState<string | null>(null);
  const [sample, setSample] = useState<string | null>(null);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const build = async (keep: boolean) => {
    setBusy(true);
    setError(undefined);
    try {
      const t = await dataApi.toTemplate(JSON.stringify(json ?? null), keep);
      setTemplate(t);
      setSample(await dataApi.templateSample(t).catch(() => null));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        className="btn btn-sm btn-ghost"
        disabled={disabled || !isTauri || json === undefined}
        onClick={() => {
          setOpen(true);
          void build(keepIds);
        }}
        title="Replace the values with random-data helpers, keeping the structure"
      >
        <Shuffle /> Randomize
      </button>
      {open && (
        <Modal
          open
          onClose={() => setOpen(false)}
          title="Randomize this body"
          size="lg"
          footer={
            <>
              <button className="btn" onClick={() => setOpen(false)}>
                Cancel
              </button>
              <button
                className="btn btn-primary"
                disabled={!template}
                onClick={() => {
                  if (template) onApply(template);
                  setOpen(false);
                }}
              >
                Use template
              </button>
            </>
          }
        >
          <div className="col">
            <div className="subtle small">
              Every response gets fresh, realistic values in the same shape. The body becomes a Handlebars template (text body with response templating on).
            </div>
            <Checkbox
              checked={keepIds}
              onChange={(v) => {
                setKeepIds(v);
                void build(v);
              }}
              label="Keep ids (id, …Id, …_id) so the same resource keeps its identity"
            />
            {error && <Callout kind="danger">{error}</Callout>}
            {busy && <Spinner />}
            {template !== null && (
              <div className="grid-2">
                <div className="col-sm">
                  <span className="label">Template</span>
                  <div className="editor-shell" style={{ height: 320 }}>
                    <CodeEditor value={template} readOnly language="handlebars" />
                  </div>
                </div>
                <div className="col-sm">
                  <span className="label">One rendering</span>
                  <div className="editor-shell" style={{ height: 320 }}>
                    <CodeEditor value={sample ?? ''} readOnly language="json" />
                  </div>
                </div>
              </div>
            )}
          </div>
        </Modal>
      )}
    </>
  );
}

/** Local-AI data shaped like the current body (an array body gets as many items as it has, 1–50). */
export function AiMockDataButton({ sample, onResult, disabled }: { sample: unknown; onResult(v: unknown): void; disabled?: boolean }) {
  const isArray = Array.isArray(sample);
  const count = isArray ? Math.min(50, Math.max(1, (sample as unknown[]).length || 5)) : 1;
  return (
    <AiAssistButton
      feature="mockData"
      label="Generate with AI"
      disabled={disabled || sample === undefined || !isTauri}
      align="right"
      input={() => ({ schemaOrSample: JSON.stringify(sample ?? {}), count })}
      onResult={(r) => onResult(isArray ? r.items : (r.items[0] ?? {}))}
      applyLabel="Use as body"
    />
  );
}
