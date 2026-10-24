import { useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@/features/mocks/lib/query';
import clsx from 'clsx';
import { FolderOpen, Upload, FilePlus2, Save, Trash2, Download, RefreshCw, FileText, FileJson2, Image as ImageIcon, Link2 } from 'lucide-react';
import { useFiles, useMappings, useWm, useWrite, qk } from '@/features/mocks/lib/queries';
import { SearchInput, Spinner, Empty, MethodBadge } from '@/features/mocks/components/ui';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { promptDialog } from '@/components/ui/dialogs';
import { downloadBlob, pickFiles } from '@/features/mocks/lib/download';
import { openNewStub, openStub } from '@/features/mocks/stubs/open';
import { displayName, stubId } from '@/features/mocks/lib/stub-utils';
import { toast } from '@/components/ui/toast';
import { SidePane } from '@/components/layout/SidePane';

const TEXT_EXT = /\.(json|txt|xml|html?|csv|ya?ml|hbs|handlebars|js|css|md|svg|graphql|gql|soap|wsdl|properties|ndjson)$/i;
const IMG_EXT = /\.(png|jpe?g|gif|webp|svg|ico|bmp)$/i;

export default function FilesPage() {
  const files = useFiles();
  const mappings = useMappings();
  const { client, sid, target } = useWm();
  const builtinTarget = target.kind === 'builtin';
  const write = useWrite();
  const qc = useQueryClient();
  const [q, setQ] = useState('');
  const [sel, setSel] = useState<string | null>(null);
  const [over, setOver] = useState(false);

  const list = useMemo(() => (files.data ?? []).filter((f) => !q || f.toLowerCase().includes(q.toLowerCase())), [files.data, q]);

  const upload = async (picked: File[], prefix = '') => {
    if (!picked.length) return;
    await write(
      async (c) => {
        for (const f of picked) await c.putFile(prefix + (f.webkitRelativePath || f.name), new Uint8Array(await f.arrayBuffer()));
      },
      { label: 'Upload files', success: `Uploaded ${picked.length} file${picked.length === 1 ? '' : 's'}`, invalidate: (s) => [qk.files(s)] },
    );
  };

  return (
    <div
      className="page"
      onDragOver={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        upload([...e.dataTransfer.files]);
      }}
    >
      <div className="page-header">
        <FolderOpen size={16} className="subtle" />
        <h1>Files</h1>
        <span className="subtle small">
          {builtinTarget ? (
            <>
              The project's <code className="code-inline">mocks/__files</code> folder
            </>
          ) : (
            <>
              The server's <code className="code-inline">__files</code> folder
            </>
          )}{' '}
          — response bodies referenced by <code className="code-inline">bodyFileName</code>.
        </span>
        <div className="spacer" />
        <button className="icon-btn" onClick={() => files.refetch()} title="Refresh">
          {files.isFetching ? <Spinner /> : <RefreshCw />}
        </button>
        <button
          className="btn btn-sm"
          onClick={async () => {
            const name = await promptDialog({ title: 'New file', label: 'Path inside __files', placeholder: 'responses/user.json', validate: (v) => (!v.trim() ? 'Required' : undefined) });
            if (!name) return;
            await write((c) => c.putFile(name.trim(), name.endsWith('.json') ? '{\n  \n}\n' : ''), { label: 'Create file', success: 'File created', invalidate: (s) => [qk.files(s)] });
            setSel(name.trim());
          }}
        >
          <FilePlus2 /> New file
        </button>
        <button className="btn btn-sm btn-primary" onClick={async () => upload(await pickFiles())}>
          <Upload /> Upload
        </button>
      </div>
      <div className="split">
        <SidePane title="Files" toolbar={<SearchInput value={q} onChange={setQ} placeholder="Filter files" className="grow" inputRef={(el) => el?.setAttribute('data-page-search', '')} />}>
          <div className={clsx('list', over && 'drop-zone over')}>
            {files.isPending && <Spinner />}
            {files.isError && <div className="p-4 text-danger small">{(files.error as Error).message}</div>}
            {files.isSuccess && list.length === 0 && (
              <div className="p-5 subtle small">
                No files. Drop files anywhere on this page to upload them, or record with “extract bodies” enabled.
              </div>
            )}
            {list.map((f) => (
              <div key={f} className={clsx('list-item', sel === f && 'on')} onClick={() => setSel(f)} title={f}>
                {IMG_EXT.test(f) ? <ImageIcon size={14} className="subtle" /> : f.endsWith('.json') ? <FileJson2 size={14} className="subtle" /> : <FileText size={14} className="subtle" />}
                <span className="mono xs truncate">{f}</span>
              </div>
            ))}
          </div>
        </SidePane>
        <div className="pane fill">
          {sel ? (
            <FileView
              key={sel}
              path={sel}
              usedBy={(mappings.data ?? []).filter((m) => m.response?.bodyFileName === sel)}
              onDelete={async () => {
                await write((c) => c.deleteFile(sel), { label: 'Delete file', success: 'File deleted', invalidate: (s) => [qk.files(s)], confirm: { title: `Delete ${sel}?`, danger: true, confirmText: 'Delete' } });
                setSel(null);
              }}
              onSaved={() => qc.invalidateQueries({ queryKey: qk.file(sid, sel) })}
              download={async () => {
                if (!client) return;
                try {
                  const bytes = await client.fileBytes(sel);
                  downloadBlob(sel.split('/').pop()!, new Blob([bytes.slice().buffer]));
                } catch (e) {
                  toast.error('Download failed', e);
                }
              }}
            />
          ) : (
            <Empty icon={<FolderOpen />} title="Select a file">
              Text files open in an editor; binary files can be downloaded or replaced.
            </Empty>
          )}
        </div>
      </div>
    </div>
  );
}

function FileView({ path, usedBy, onDelete, onSaved, download }: { path: string; usedBy: import('@/features/mocks/lib/types').StubMapping[]; onDelete(): void; onSaved(): void; download(): void }) {
  const { client, sid, server } = useWm();
  const write = useWrite();
  const isText = TEXT_EXT.test(path) || !/\.[a-z0-9]+$/i.test(path);
  const isImg = IMG_EXT.test(path);
  const content = useQuery({ queryKey: qk.file(sid, path), queryFn: () => client!.fileText(path), enabled: !!client && isText });
  const bytes = useQuery({ queryKey: [...qk.file(sid, path), 'bytes'], queryFn: () => client!.fileBytes(path), enabled: !!client && isImg && !isText });
  const [text, setText] = useState<string | null>(null);
  useEffect(() => setText(null), [path]);
  const value = text ?? content.data ?? '';
  const dirty = text !== null && text !== content.data;
  const imgUrl = useMemo(() => (bytes.data ? URL.createObjectURL(new Blob([bytes.data.slice().buffer])) : undefined), [bytes.data]);

  const save = async () => {
    if (!dirty) return;
    await write((c) => c.putFile(path, value), { label: 'Save file', success: 'File saved', invalidate: (s) => [qk.file(s, path)] });
    setText(null);
    onSaved();
  };

  return (
    <div className="page">
      <div className="toolbar">
        <span className="mono small truncate grow">{path}</span>
        {dirty && <span className="subtle xs">Unsaved</span>}
        <button className="btn btn-sm" onClick={download}>
          <Download /> Download
        </button>
        <button
          className="btn btn-sm"
          onClick={async () => {
            const [f] = await pickFiles(undefined, false);
            if (f) await write(async (c) => c.putFile(path, new Uint8Array(await f.arrayBuffer())), { label: 'Replace file', success: 'File replaced', invalidate: (s) => [qk.file(s, path)] });
          }}
        >
          <Upload /> Replace
        </button>
        <button
          className="btn btn-sm"
          onClick={() =>
            openNewStub({
              name: `Serve ${path}`,
              request: { method: 'GET', urlPath: `/${path}` },
              response: { status: 200, headers: { 'Content-Type': mime(path) }, bodyFileName: path },
            })
          }
        >
          <Link2 /> Stub that serves it
        </button>
        {isText && (
          <button className="btn btn-sm btn-primary" disabled={!dirty || server?.readOnly} onClick={save}>
            <Save /> Save
          </button>
        )}
        <button className="icon-btn sm danger" onClick={onDelete} title="Delete file">
          <Trash2 />
        </button>
      </div>
      {usedBy.length > 0 && (
        <div className="toolbar small">
          <span className="subtle xs">Used by</span>
          {usedBy.map((m) => (
            <button key={stubId(m)} className="btn btn-sm btn-ghost" onClick={() => openStub(m)}>
              <MethodBadge method={m.request.method} /> {displayName(m)}
            </button>
          ))}
        </div>
      )}
      <div className="grow min-h-0 p-4" style={{ display: 'flex', flexDirection: 'column' }}>
        {isText ? (
          content.isPending ? (
            <Spinner />
          ) : content.isError ? (
            <div className="text-danger">{(content.error as Error).message}</div>
          ) : (
            <div className="editor-shell fill">
              <CodeEditor value={value} onChange={setText} language={languageFor(path, value)} onSave={save} />
            </div>
          )
        ) : isImg && imgUrl ? (
          <div style={{ display: 'grid', placeItems: 'center', flex: 1 }}>
            <img src={imgUrl} alt={path} style={{ maxWidth: '100%', maxHeight: '70vh', border: '1px solid var(--border)', borderRadius: 8 }} />
          </div>
        ) : (
          <Empty title="Binary file">Download it, or replace it with a new upload.</Empty>
        )}
      </div>
    </div>
  );
}

function mime(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  return (
    {
      json: 'application/json',
      xml: 'application/xml',
      html: 'text/html',
      htm: 'text/html',
      txt: 'text/plain',
      csv: 'text/csv',
      png: 'image/png',
      jpg: 'image/jpeg',
      jpeg: 'image/jpeg',
      gif: 'image/gif',
      svg: 'image/svg+xml',
      pdf: 'application/pdf',
      js: 'application/javascript',
      css: 'text/css',
      yaml: 'application/yaml',
      yml: 'application/yaml',
    }[ext] ?? 'application/octet-stream'
  );
}
