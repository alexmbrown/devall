import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, BookOpen, ChevronDown, Wand2, X } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { monaco } from '@/components/editor/monaco';
import { Menu } from '@/components/ui/Menu';
import { errorLocation, normalizeScripts, PHASE_LABEL, scriptsApi, type ScriptError, type ScriptPhase, type Scripts } from '@/lib/api/scripts';
import { ScriptHelp } from './ScriptHelp';
import { SCRIPT_MODEL_PREFIX, registerScriptCompletions } from './completions';
import { appendSnippet, snippetsFor } from './snippets';
import './scripts.css';

export interface ScriptsEditorProps {
  scripts: Scripts | undefined;
  /** Receives `undefined` when both scripts are empty (so nothing is written to the file). */
  onChange(scripts: Scripts | undefined): void;
  /** Folder scripts run for every request below the folder (outermost folder first). */
  folder?: boolean;
  /** Keeps Monaco models apart per request/folder (e.g. the tab id or path). */
  modelKey?: string;
}

const KEY: Record<ScriptPhase, keyof Scripts> = { preRequest: 'preRequest', postResponse: 'postResponse' };

const DESCRIPTION: Record<ScriptPhase, { request: string; folder: string }> = {
  preRequest: {
    request: 'Runs before {{variables}} are resolved — edit the request with pm.request, set variables with pm.variables.set.',
    folder: 'Runs before every request in this folder (outer folders first, then the request’s own script).',
  },
  postResponse: {
    request: 'Runs after the response — add pm.test(…) checks and store values with pm.environment.set.',
    folder: 'Runs after every response in this folder (outer folders first, then the request’s own script).',
  },
};

/** Pre-request and post-response script editors with snippets, autocomplete and the API reference. */
export function ScriptsEditor({ scripts, onChange, folder, modelKey = 'draft' }: ScriptsEditorProps) {
  const [help, setHelp] = useState(false);
  useEffect(registerScriptCompletions, []);
  const set = (phase: ScriptPhase, code: string) => onChange(normalizeScripts({ ...scripts, [KEY[phase]]: code }));
  const safeKey = modelKey.replace(/[^\w.-]+/g, '_');

  return (
    <div className="scripts-editor">
      <div className={clsx('scripts-panes', help && 'with-help')}>
        <div className="scripts-stack">
          {(['preRequest', 'postResponse'] as ScriptPhase[]).map((phase) => (
            <ScriptPane
              key={phase}
              phase={phase}
              value={scripts?.[KEY[phase]] ?? ''}
              onChange={(code) => set(phase, code)}
              description={DESCRIPTION[phase][folder ? 'folder' : 'request']}
              modelPath={`${SCRIPT_MODEL_PREFIX}/${folder ? 'folder' : 'request'}/${safeKey}/${phase}.js`}
              helpOpen={help}
              onToggleHelp={() => setHelp((h) => !h)}
            />
          ))}
        </div>
        {help && (
          <aside className="scripts-help-pane">
            <div className="scripts-help-head">
              <BookOpen />
              <strong className="small">Script API</strong>
              <span className="spacer" />
              <button className="icon-btn" onClick={() => setHelp(false)} aria-label="Close the API reference">
                <X />
              </button>
            </div>
            <ScriptHelp />
          </aside>
        )}
      </div>
    </div>
  );
}

function ScriptPane({
  phase,
  value,
  onChange,
  description,
  modelPath,
  helpOpen,
  onToggleHelp,
}: {
  phase: ScriptPhase;
  value: string;
  onChange(v: string): void;
  description: string;
  modelPath: string;
  helpOpen: boolean;
  onToggleHelp(): void;
}) {
  const syntax = useSyntaxCheck(value, modelPath);
  const lines = value.trim() ? value.split('\n').length : 0;
  return (
    <section className="script-pane">
      <header className="script-pane-head">
        <strong className="small">{PHASE_LABEL[phase]}</strong>
        <span className="subtle xs truncate" title={description}>
          {description}
        </span>
        <span className="spacer" />
        {lines > 0 && <span className="badge">{lines} {lines === 1 ? 'line' : 'lines'}</span>}
        <Menu
          align="right"
          items={[
            { heading: 'Append to the script' },
            ...snippetsFor(phase).map((s) => ({
              label: (
                <>
                  {s.label}
                  <span className="menu-desc">{s.hint}</span>
                </>
              ),
              onSelect: () => onChange(appendSnippet(value, s.code)),
            })),
          ]}
          trigger={(p) => (
            <button className="btn btn-sm" {...p} title="Insert a snippet at the end of the script">
              <Wand2 /> Snippets <ChevronDown />
            </button>
          )}
        />
        <button className={clsx('btn btn-sm', helpOpen && 'on')} onClick={onToggleHelp} title="Supported pm API" aria-pressed={helpOpen}>
          <BookOpen /> API
        </button>
      </header>
      <div className={clsx('script-pane-editor', syntax && 'has-error')}>
        <CodeEditor value={value} onChange={onChange} language="javascript" path={modelPath} />
      </div>
      {syntax && (
        <div className="script-syntax-error" role="status">
          <AlertTriangle />
          <span className="truncate" title={syntax.message}>
            {syntax.line != null && <span className="mono">line {syntax.line}{syntax.column != null ? `:${syntax.column}` : ''} · </span>}
            {syntax.message}
          </span>
        </div>
      )}
    </section>
  );
}

let checkUnavailable = false;

/** Debounced `script_check`; marks the error in the editor. Silent when the command isn't registered. */
function useSyntaxCheck(code: string, modelPath: string): ScriptError | null {
  const [error, setError] = useState<ScriptError | null>(null);
  const seq = useRef(0);
  useEffect(() => {
    const n = ++seq.current;
    if (checkUnavailable || !code.trim()) {
      setError(null);
      setMarkers(modelPath, null);
      return;
    }
    const t = setTimeout(() => {
      scriptsApi
        .check(code)
        .then((e) => {
          if (n !== seq.current) return;
          setError(e);
          setMarkers(modelPath, e);
        })
        .catch(() => {
          checkUnavailable = true;
        });
    }, 450);
    return () => clearTimeout(t);
  }, [code, modelPath]);
  useEffect(() => () => setMarkers(modelPath, null), [modelPath]);
  return error;
}

function setMarkers(modelPath: string, e: ScriptError | null) {
  const model = monaco.editor.getModels().find((m) => m.uri.path.replace(/^\//, '') === modelPath);
  if (!model) return;
  if (!e || e.line == null) {
    monaco.editor.setModelMarkers(model, 'devall-script', []);
    return;
  }
  const line = Math.min(e.line, model.getLineCount());
  const col = Math.max(1, e.column ?? 1);
  monaco.editor.setModelMarkers(model, 'devall-script', [
    {
      severity: monaco.MarkerSeverity.Error,
      message: e.message,
      startLineNumber: line,
      startColumn: col,
      endLineNumber: line,
      endColumn: Math.max(col + 1, model.getLineMaxColumn(line)),
      source: errorLocation(e),
    },
  ]);
}
