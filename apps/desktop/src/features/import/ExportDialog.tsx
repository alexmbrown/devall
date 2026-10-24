import { useEffect, useMemo, useState } from 'react';
import { Download, FolderOpen } from 'lucide-react';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { dirname, join } from '@tauri-apps/api/path';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, CopyButton, Field, Segmented, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { CodeEditor, type CodeLanguage } from '@/components/editor/CodeEditor';
import { errorMessage } from '@/lib/ipc';
import type { TreeNode } from '@/lib/types';
import { exportFor, importApi, type ExportFormat, type ExportOutput } from '@/lib/api/import';
import { useWorkspace } from '@/store/workspace';
import './import.css';

export interface ExportDialogProps {
  open: boolean;
  onClose(): void;
  /** Folder (relative to `http/`) to export; '' or undefined = whole project. */
  folder?: string;
  format?: ExportFormat;
}

const FORMATS: { value: ExportFormat; label: string; title: string }[] = [
  { value: 'openapi', label: 'OpenAPI 3.1', title: 'YAML spec: paths, parameters, bodies, security schemes, servers from environments' },
  { value: 'postman', label: 'Postman v2.1', title: 'Collection JSON plus one environment file per environment' },
  { value: 'http', label: '.http', title: 'JetBrains HTTP Client / VS Code REST Client file plus http-client.env.json' },
  { value: 'hurl', label: 'Hurl', title: 'Hurl file (asserts and captures from your tests) plus one --variables-file per environment' },
];

const FILTERS: Record<ExportFormat, { name: string; extensions: string[] }> = {
  openapi: { name: 'OpenAPI (YAML)', extensions: ['yaml', 'yml'] },
  postman: { name: 'Postman collection', extensions: ['json'] },
  http: { name: 'HTTP file', extensions: ['http', 'rest'] },
  hurl: { name: 'Hurl file', extensions: ['hurl'] },
};

function folderOptions(tree: TreeNode | null): { path: string; label: string }[] {
  const out = [{ path: '', label: 'Whole project' }];
  const walk = (n: TreeNode, depth: number) => {
    if (n.kind !== 'folder') return;
    for (const c of n.children) {
      if (c.kind === 'folder') {
        out.push({ path: c.path, label: `${'  '.repeat(depth)}${c.name}` });
        walk(c, depth + 1);
      }
    }
  };
  if (tree) walk(tree, 1);
  return out;
}

function editorLanguage(lang: string): CodeLanguage {
  return lang === 'yaml' ? 'yaml' : lang === 'json' ? 'json' : 'plaintext';
}

/** Export a project or folder as OpenAPI 3.1, Postman v2.1, a .http file or Hurl: preview, copy, save. */
export function ExportDialog({ open, onClose, folder = '', format: initialFormat = 'openapi' }: ExportDialogProps) {
  const project = useWorkspace((s) => s.project);
  const tree = useWorkspace((s) => s.tree);
  const [format, setFormat] = useState<ExportFormat>(initialFormat);
  const [scope, setScope] = useState(folder);
  const [env, setEnv] = useState('');
  const [out, setOut] = useState<ExportOutput | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [withAttachments, setWithAttachments] = useState(true);
  const folders = useMemo(() => folderOptions(tree), [tree]);

  useEffect(() => {
    if (!open) return;
    setScope(folder);
    setFormat(initialFormat);
  }, [open, folder, initialFormat]);

  useEffect(() => {
    if (!open || !project) return;
    let stale = false;
    setBusy(true);
    setError(null);
    exportFor(format, project.root, scope || undefined, format === 'openapi' && env ? env : undefined)
      .then((o) => !stale && setOut(o))
      .catch((e) => {
        if (stale) return;
        setOut(null);
        setError(errorMessage(e));
      })
      .finally(() => !stale && setBusy(false));
    return () => {
      stale = true;
    };
  }, [open, project, format, scope, env]);

  const saveFile = async () => {
    if (!out) return;
    try {
      const dest = await saveDialog({ defaultPath: out.fileName, filters: [FILTERS[format], { name: 'All files', extensions: ['*'] }] });
      if (!dest) return;
      await importApi.exportWrite(dest, out.content);
      let extra = 0;
      if (withAttachments && out.attachments.length) {
        const dir = await dirname(dest);
        for (const a of out.attachments) {
          await importApi.exportWrite(await join(dir, a.fileName), a.content);
          extra++;
        }
      }
      toast.success(extra ? `Saved ${out.fileName} and ${extra} companion file${extra === 1 ? '' : 's'}` : 'Export saved', dest, {
        label: 'Reveal',
        run: () => revealItemInDir(dest).catch(() => {}),
      });
    } catch (e) {
      toast.error('Could not save the export', errorMessage(e));
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Export"
      size="xl"
      footer={
        <>
          {out && out.attachments.length > 0 && (
            <Checkbox
              checked={withAttachments}
              onChange={setWithAttachments}
              label={`Also save ${out.attachments.map((a) => a.fileName).join(', ')}`}
              title="Written next to the main file. Secret values are never exported."
            />
          )}
          <span className="grow" />
          {out && <CopyButton text={out.content} label="Copy" />}
          <button className="btn btn-primary" onClick={saveFile} disabled={!out || busy}>
            <Download /> Save…
          </button>
        </>
      }
    >
      {!project ? (
        <Callout kind="warning">Open a project first.</Callout>
      ) : (
        <div className="exp-layout">
          <div className="imp-row">
            <Segmented value={format} onChange={setFormat} options={FORMATS} />
            <Field className="grow">
              <select className="select sm" value={scope} onChange={(e) => setScope(e.target.value)} aria-label="Scope">
                {folders.map((f) => (
                  <option key={f.path} value={f.path}>
                    {f.label}
                  </option>
                ))}
              </select>
            </Field>
            {format === 'openapi' && project.environments.length > 0 && (
              <select className="select sm" value={env} onChange={(e) => setEnv(e.target.value)} aria-label="Servers" title="Which environments' baseUrl become servers">
                <option value="">Servers: all environments</option>
                {project.environments.map((e) => (
                  <option key={e.id} value={e.id}>
                    Servers: {e.file.name}
                  </option>
                ))}
              </select>
            )}
            {busy && <Spinner />}
            {out && (
              <span className="small muted mono" title="Suggested file name">
                <FolderOpen size={12} /> {out.fileName}
              </span>
            )}
          </div>
          {error && <Callout kind="danger">{error}</Callout>}
          {out && out.warnings.length > 0 && (
            <Callout kind="warning">
              <ul className="imp-list">
                {out.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            </Callout>
          )}
          <div className="exp-preview">
            <CodeEditor value={out?.content ?? ''} language={editorLanguage(out?.language ?? '')} readOnly path={`export-preview.${format}`} />
          </div>
        </div>
      )}
    </Modal>
  );
}
