import { useEffect, useState } from 'react';
import { Check, Eye, FileCode2, Save, Server, ShieldCheck, Sparkles, X } from 'lucide-react';
import { CodeDiff, CodeEditor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { proxyApi, type ExchangeId, type StubOptions } from '@/lib/api/proxy';
import { aiApi, type OpenApiNamingView, type Preview } from '@/lib/api/ai';
import { openapiApi, type LintReport } from '@/lib/api/openapi';
import { MetaLine, PreviewView } from '@/features/ai/AiAssistButton';
import { newCallId, useAi } from '@/features/ai/store';
import { LintReportView } from '@/features/openapi/reports';
import '@/features/openapi/openapi.css';
import { useWorkspace } from '@/store/workspace';

/** Selected exchanges → WireMock stubs in `mocks/mappings/`. */
export function StubsDialog({ open, onClose, root, ids }: { open: boolean; onClose(): void; root: string; ids: ExchangeId[] }) {
  const [o, setO] = useState<Required<Omit<StubOptions, 'matchHeaders'>> & { headers: string }>({
    matchQuery: true,
    body: 'equalToJson',
    ignoreExtraElements: true,
    generalize: true,
    includeSensitive: false,
    folder: '',
    headers: '',
  });
  const [busy, setBusy] = useState(false);

  const run = async () => {
    setBusy(true);
    try {
      const { headers, ...rest } = o;
      const res = await proxyApi.toStubs(root, ids, { ...rest, matchHeaders: headers.split(',').map((h) => h.trim()).filter(Boolean) });
      toast.success(`${res.written.length} stub${res.written.length === 1 ? '' : 's'} written`, res.written.slice(0, 4).map((w) => w.file).join('\n') + (res.skipped.length ? `\nSkipped ${res.skipped.length}: ${res.skipped[0]}` : ''));
      void useWorkspace.getState().refresh();
      onClose();
    } catch (e) {
      toast.error('Could not create stubs', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={
        <span className="row-sm">
          <Server size={16} /> Create mock stubs
        </span>
      }
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={run} disabled={busy}>
            Create {ids.length} stub{ids.length === 1 ? '' : 's'}
          </button>
        </>
      }
    >
      <div className="col">
        <p className="small muted">
          One WireMock mapping per exchange in <code className="code-inline">mocks/mappings/</code> — served by the /dev/all mock server or any WireMock. JSON responses become <code className="code-inline">jsonBody</code>.
        </p>
        <Checkbox checked={o.generalize} onChange={(v) => setO({ ...o, generalize: v })} label="Generalize ids in paths (/users/42 → /users/[0-9]+, UUIDs, hashes…)" />
        <Checkbox checked={o.matchQuery} onChange={(v) => setO({ ...o, matchQuery: v })} label="Match query parameters" />
        <div className="grid-2">
          <Field label="Request body">
            <select className="select sm" value={o.body} onChange={(e) => setO({ ...o, body: e.target.value as StubOptions['body'] & string })}>
              <option value="equalToJson">Match JSON (ignore extra fields)</option>
              <option value="equalTo">Match exact text</option>
              <option value="none">Don't match the body</option>
            </select>
          </Field>
          <Field label="Sub-folder of mocks/mappings" hint="Optional">
            <input className="input sm mono" value={o.folder} placeholder="captured" onChange={(e) => setO({ ...o, folder: e.target.value })} />
          </Field>
        </div>
        <Field label="Request headers to match" hint="Comma-separated, e.g. x-tenant, accept">
          <input className="input sm mono" value={o.headers} onChange={(e) => setO({ ...o, headers: e.target.value })} />
        </Field>
        <Checkbox checked={o.includeSensitive} onChange={(v) => setO({ ...o, includeSensitive: v })} label="Keep Set-Cookie / Authorization values (they will be committed to the project)" />
      </div>
    </Modal>
  );
}

/** Selected exchanges → OpenAPI 3.1 draft, previewed and editable before saving to the project.
 * The deterministic draft is the default; "Improve names with AI" proposes operationIds,
 * summaries, tags and schema names as a reviewable diff; "Check spec" lints whatever is in the
 * editor. */
export function OpenApiDialog({ open, onClose, root, ids, defaultName }: { open: boolean; onClose(): void; root: string; ids: ExchangeId[]; defaultName: string }) {
  const [yaml, setYaml] = useState<string | null>(null);
  const [path, setPath] = useState(`openapi/${defaultName}.openapi.yaml`);
  const [examples, setExamples] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lint, setLint] = useState<LintReport | null>(null);
  const [linting, setLinting] = useState(false);
  const [ai, setAi] = useState<OpenApiNamingView | null>(null);
  const [aiBusy, setAiBusy] = useState<string | null>(null);
  const [aiError, setAiError] = useState<string | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const selected = useAi((s) => s.selected);

  useEffect(() => {
    if (!open) return;
    setYaml(null);
    setError(null);
    setLint(null);
    setAi(null);
    setAiError(null);
    setPreview(null);
    setPath(`openapi/${defaultName}.openapi.yaml`);
    proxyApi
      .toOpenApi(root, ids, { includeExamples: examples })
      .then(setYaml)
      .catch((e) => setError(errorMessage(e)));
  }, [open, root, ids, examples, defaultName]);

  const check = async (text = yaml) => {
    if (!text) return;
    setLinting(true);
    try {
      setLint(await openapiApi.lintText(text));
    } catch (e) {
      toast.error('Lint failed', errorMessage(e));
    } finally {
      setLinting(false);
    }
  };

  const improve = async () => {
    if (!yaml) return;
    const id = newCallId();
    setAiBusy(id);
    setAiError(null);
    setPreview(null);
    try {
      setAi(await aiApi.openapiNames(root, yaml, { provider: selected, callId: id }));
    } catch (e) {
      const m = errorMessage(e);
      if (m !== 'cancelled') setAiError(m);
    } finally {
      setAiBusy(null);
    }
  };

  const showPreview = async () => {
    if (!yaml) return;
    setAiError(null);
    try {
      setPreview(await aiApi.preview(root, 'openapiNames', { spec: yaml }, selected));
    } catch (e) {
      setAiError(errorMessage(e));
    }
  };

  const accept = () => {
    if (!ai) return;
    setYaml(ai.improved);
    setLint(ai.lintAfter);
    setAi(null);
    toast.success('Names applied', 'Review the draft, then save it to the project.');
  };

  const save = async () => {
    if (!yaml) return;
    try {
      const rel = await proxyApi.saveOpenApi(root, path, yaml);
      toast.success('OpenAPI draft saved', rel);
      void useWorkspace.getState().refresh();
      onClose();
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      size="xl"
      title={
        <span className="row-sm">
          <FileCode2 size={16} /> OpenAPI draft
        </span>
      }
      bodyClassName="insp-openapi-body"
      footer={
        ai ? (
          <>
            <span className="grow small muted">{ai.changes.length} change{ai.changes.length === 1 ? '' : 's'} proposed — nothing is applied until you accept them.</span>
            <button className="btn" onClick={() => setAi(null)}>
              <X /> Discard
            </button>
            <button className="btn btn-primary" onClick={accept}>
              <Check /> Use these names
            </button>
          </>
        ) : (
          <>
            <Field className="grow">
              <input className="input sm mono" value={path} onChange={(e) => setPath(e.target.value)} aria-label="Save to (relative to the project)" />
            </Field>
            <button className="btn" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-primary" onClick={save} disabled={!yaml}>
              <Save /> Save to project
            </button>
          </>
        )
      }
    >
      <div className="col-sm h-full">
        <div className="row wrap">
          <span className="small muted grow">
            Inferred deterministically from {ids.length} exchange{ids.length === 1 ? '' : 's'}: paths clustered, ids turned into parameters, JSON schemas merged with required fields. Review it before publishing — you can edit it here.
          </span>
          <Checkbox checked={examples} onChange={setExamples} label="Include examples (may contain personal data)" />
        </div>
        <div className="row-sm wrap">
          <button className="btn btn-sm" onClick={() => void check()} disabled={!yaml || linting} title="Lint the spec in the editor (built-in rules, no network)">
            {linting ? <Spinner /> : <ShieldCheck />} Check spec
          </button>
          {aiBusy ? (
            <>
              <Spinner /> <span className="small subtle">Naming operations and schemas…</span>
              <button className="btn btn-sm" onClick={() => void aiApi.cancel(aiBusy)}>
                Cancel
              </button>
            </>
          ) : (
            <>
              <button className="btn btn-sm" onClick={() => void improve()} disabled={!yaml || !!ai} title="Propose operationIds, summaries, tags and schema names with your configured model (reviewed as a diff before anything changes)">
                <Sparkles /> Improve names with AI
              </button>
              <button className="btn btn-sm btn-ghost" onClick={() => void showPreview()} disabled={!yaml} title="Show exactly what will be sent (an outline: names and shapes, no example values)">
                <Eye /> What will be sent
              </button>
            </>
          )}
        </div>
        {error && <Callout kind="danger">{error}</Callout>}
        {aiError && <Callout kind="danger">{aiError}</Callout>}
        {preview && <PreviewView preview={preview} compact />}
        {ai ? (
          <div className="col-sm grow min-h-0">
            <div className="row-sm wrap small">
              <span className="strong">Proposed by {ai.meta.model}</span>
              <span className="subtle">
                lint: {ai.lintBefore.errors}/{ai.lintBefore.warnings} → {ai.lintAfter.errors}/{ai.lintAfter.warnings} (errors/warnings)
              </span>
              {ai.problem && <span className="text-warning xs">{ai.problem}</span>}
            </div>
            {ai.changes.length > 0 && (
              <details>
                <summary className="small subtle">Changes ({ai.changes.length})</summary>
                <ul className="xs mono" style={{ margin: 0, maxHeight: 120, overflow: 'auto' }}>
                  {ai.changes.map((c, i) => (
                    <li key={i}>{c}</li>
                  ))}
                </ul>
              </details>
            )}
            <div className="grow min-h-0 insp-openapi-editor">
              <CodeDiff original={yaml ?? ''} modified={ai.improved} language="yaml" />
            </div>
            <MetaLine meta={ai.meta} />
          </div>
        ) : (
          <div className="grow min-h-0 insp-openapi-editor">
            {yaml == null ? (
              <div className="empty">
                <Spinner />
              </div>
            ) : (
              <CodeEditor value={yaml} onChange={setYaml} language="yaml" path="openapi-draft.yaml" />
            )}
          </div>
        )}
        {lint && !ai && (
          <div style={{ maxHeight: 220, overflow: 'auto' }}>
            <LintReportView report={lint} />
          </div>
        )}
      </div>
    </Modal>
  );
}
