import { FolderOpen, Trash2, Wand2 } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Checkbox, Field } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { KeyValueEditor } from './KeyValueEditor';
import type { Body, BodyType, MultipartPart, VarInfo } from '@/lib/types';

const TYPES: { value: BodyType; label: string }[] = [
  { value: 'none', label: 'None' },
  { value: 'json', label: 'JSON' },
  { value: 'text', label: 'Text' },
  { value: 'xml', label: 'XML' },
  { value: 'form', label: 'Form URL-encoded' },
  { value: 'multipart', label: 'Multipart form' },
  { value: 'graphql', label: 'GraphQL' },
  { value: 'binary', label: 'Binary file' },
];

/** Converts between body types, keeping text content where it makes sense. */
function convert(body: Body, type: BodyType): Body {
  const text = 'content' in body ? body.content : '';
  switch (type) {
    case 'none':
      return { type };
    case 'json':
      return { type, content: text || '{\n  \n}' };
    case 'text':
      return { type, content: text };
    case 'xml':
      return { type, content: text };
    case 'form':
      return { type, fields: body.type === 'multipart' ? body.parts.filter((p) => !p.file).map((p) => ({ name: p.name, value: p.value })) : [] };
    case 'multipart':
      return { type, parts: body.type === 'form' ? body.fields.map((f) => ({ name: f.name, value: f.value })) : [] };
    case 'graphql':
      return { type, query: 'query {\n  \n}', variables: '' };
    case 'binary':
      return { type, path: '' };
  }
}

export function BodyEditor({ body, onChange, vars, projectRoot }: { body: Body; onChange(b: Body): void; vars: VarInfo[]; projectRoot: string }) {
  const pickFile = async () => {
    const f = await openDialog({ multiple: false, directory: false, defaultPath: projectRoot });
    if (typeof f !== 'string') return null;
    // Store project-relative paths so the file works for teammates too.
    const norm = (s: string) => s.replace(/\\/g, '/');
    const root = norm(projectRoot).replace(/\/$/, '');
    return norm(f).startsWith(`${root}/`) ? norm(f).slice(root.length + 1) : f;
  };

  return (
    <div className="col h-full min-h-0">
      <div className="row">
        <select className="select sm" style={{ width: 200 }} value={body.type} onChange={(e) => onChange(convert(body, e.target.value as BodyType))}>
          {TYPES.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
        {body.type === 'json' && (
          <button
            className="btn btn-sm btn-ghost"
            title="Pretty-print (templates like {{id}} must be inside strings)"
            onClick={() => {
              try {
                onChange({ ...body, content: JSON.stringify(JSON.parse(body.content), null, 2) });
              } catch (e) {
                toast.error('Not valid JSON', (e as Error).message);
              }
            }}
          >
            <Wand2 /> Format
          </button>
        )}
        <span className="spacer" />
        {['json', 'text', 'xml', 'graphql'].includes(body.type) && <span className="subtle xs">Use {'{{variables}}'} anywhere</span>}
      </div>

      {body.type === 'none' && <p className="muted small">This request has no body.</p>}
      {(body.type === 'json' || body.type === 'text' || body.type === 'xml') && (
        <div className="editor-shell fill">
          <CodeEditor value={body.content} onChange={(content) => onChange({ ...body, content })} language={body.type === 'json' ? 'json' : body.type === 'xml' ? 'xml' : 'plaintext'} />
        </div>
      )}
      {body.type === 'text' && (
        <Field label="Content-Type">
          <input className="input sm mono" value={body.contentType ?? ''} placeholder="text/plain; charset=utf-8" onChange={(e) => onChange({ ...body, contentType: e.target.value || undefined })} />
        </Field>
      )}
      {body.type === 'form' && <KeyValueEditor rows={body.fields} onChange={(fields) => onChange({ ...body, fields })} vars={vars} />}
      {body.type === 'multipart' && <MultipartEditor parts={body.parts} onChange={(parts) => onChange({ ...body, parts })} vars={vars} pickFile={pickFile} />}
      {body.type === 'graphql' && (
        <div className="graphql-split">
          <div className="col-sm min-h-0">
            <span className="label">Query</span>
            <div className="editor-shell fill">
              <CodeEditor value={body.query} onChange={(query) => onChange({ ...body, query })} language="plaintext" />
            </div>
          </div>
          <div className="col-sm min-h-0">
            <span className="label">Variables (JSON)</span>
            <div className="editor-shell fill">
              <CodeEditor value={body.variables ?? ''} onChange={(variables) => onChange({ ...body, variables })} language="json" />
            </div>
          </div>
        </div>
      )}
      {body.type === 'binary' && (
        <div className="col" style={{ maxWidth: 640 }}>
          <Field label="File" hint="Paths inside the project are stored relative to it, so they work for everyone who clones the repo.">
            <div className="row-sm">
              <VarInput value={body.path} onChange={(path) => onChange({ ...body, path })} vars={vars} placeholder="fixtures/upload.png" />
              <button
                className="btn btn-sm"
                onClick={async () => {
                  const p = await pickFile();
                  if (p) onChange({ ...body, path: p });
                }}
              >
                <FolderOpen /> Choose…
              </button>
            </div>
          </Field>
          <Field label="Content-Type">
            <input className="input sm mono" value={body.contentType ?? ''} placeholder="application/octet-stream" onChange={(e) => onChange({ ...body, contentType: e.target.value || undefined })} />
          </Field>
        </div>
      )}
    </div>
  );
}

function MultipartEditor({ parts, onChange, vars, pickFile }: { parts: MultipartPart[]; onChange(p: MultipartPart[]): void; vars: VarInfo[]; pickFile(): Promise<string | null> }) {
  const list: MultipartPart[] = [...parts, { name: '', value: '' }];
  const update = (i: number, patch: Partial<MultipartPart>) => {
    const next = list.map((p, j) => (j === i ? { ...p, ...patch } : p));
    onChange(next.filter((p, j) => j < next.length - 1 || p.name || p.value || p.file));
  };
  return (
    <div className="kv-editor">
      {list.map((p, i) => {
        const blank = i === list.length - 1;
        return (
          <div key={i} className={`kv-row multipart${p.enabled === false ? ' off' : ''}`}>
            <span className="kv-check">{!blank && <Checkbox checked={p.enabled !== false} onChange={(v) => update(i, { enabled: v ? undefined : false })} />}</span>
            <input className="input sm mono" value={p.name} placeholder="Field" onChange={(e) => update(i, { name: e.target.value })} />
            {p.file !== undefined ? (
              <div className="row-sm">
                <input className="input sm mono" value={p.file} onChange={(e) => update(i, { file: e.target.value })} />
                <button className="btn btn-sm btn-ghost" onClick={() => update(i, { file: undefined })} title="Use a text value instead">
                  Text
                </button>
              </div>
            ) : (
              <div className="row-sm">
                <VarInput value={p.value} onChange={(value) => update(i, { value })} vars={vars} placeholder="Value" className="sm" />
                <button
                  className="btn btn-sm btn-ghost"
                  title="Upload a file"
                  onClick={async () => {
                    const f = await pickFile();
                    if (f) update(i, { file: f, value: '' });
                  }}
                >
                  <FolderOpen /> File
                </button>
              </div>
            )}
            <input className="input sm mono" value={p.contentType ?? ''} placeholder="Content-Type" onChange={(e) => update(i, { contentType: e.target.value || undefined })} />
            <span className="kv-actions">
              {!blank && (
                <button className="icon-btn sm danger" onClick={() => onChange(parts.filter((_, j) => j !== i))} aria-label="Remove part">
                  <Trash2 />
                </button>
              )}
            </span>
          </div>
        );
      })}
    </div>
  );
}
