import { useMemo, useState } from 'react';
import { Server } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, Segmented, Spinner } from '@/components/ui/misc';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { toast } from '@/components/ui/toast';
import { errorMessage, isTauri } from '@/lib/ipc';
import { useWorkspace } from '@/store/workspace';
import { useNav as useAppNav } from '@/store/nav';
import type { SendResult } from '@/lib/types';
import { DEFAULT_FROM_RESPONSE, randomizeStub, saveStubToProject, stubFromSendResult, type FromResponseOptions } from './fromResponse';
import { prettyJson } from './lib/stub-utils';
import { useNav } from './store/nav';
import { useMockTarget, BUILTIN_ID } from './store/target';
import { openStub } from './stubs/open';

/**
 * "Mock this response": creates a stub of the project's mock server from an HTTP-client result
 * (method + path [+ query] → status, headers and body), optionally with randomized data.
 * Mount it in the HTTP response panel's toolbar: `<MockFromResponseButton result={tab.result} />`.
 */
export function MockFromResponseButton({ result, root: rootProp, className, compact }: { result?: SendResult | null; root?: string; className?: string; compact?: boolean }) {
  const project = useWorkspace((s) => s.project);
  const root = rootProp ?? project?.root;
  const [open, setOpen] = useState(false);
  const usable = !!root && !!result?.sent && !!result.response;
  return (
    <>
      <button
        className={className ?? 'btn btn-sm btn-ghost'}
        disabled={!usable}
        onClick={() => setOpen(true)}
        title={usable ? 'Create a mock-server stub that returns this response' : 'Send the request first'}
        aria-label="Mock this response"
      >
        <Server /> {!compact && 'Mock this'}
      </button>
      {open && result && root && <MockDialog result={result} root={root} onClose={() => setOpen(false)} />}
    </>
  );
}

function MockDialog({ result, root, onClose }: { result: SendResult; root: string; onClose(): void }) {
  const [o, setO] = useState<FromResponseOptions>(DEFAULT_FROM_RESPONSE);
  const [busy, setBusy] = useState(false);
  const built = useMemo(() => {
    try {
      return { stub: stubFromSendResult(result, o) };
    } catch (e) {
      return { error: errorMessage(e) };
    }
  }, [result, o]);
  const isJson = !!built.stub && (built.stub.response.jsonBody !== undefined || /json/i.test(result.response?.contentType ?? ''));

  const create = async () => {
    if (!built.stub) return;
    setBusy(true);
    try {
      const stub = o.randomize ? await randomizeStub(built.stub, o.keepIds) : built.stub;
      const r = await saveStubToProject(root, stub);
      onClose();
      const show = async () => {
        await useMockTarget.getState().init(root);
        const st = useMockTarget.getState();
        st.setActive(BUILTIN_ID);
        useAppNav.getState().go('mocks');
        if (r.live) openStub(r.stub);
        else useNav.getState().go('stubs');
      };
      if (r.live) toast.success('Stub created and served', `${r.stub.request.method} ${r.url}${built.stub.request.urlPath ?? built.stub.request.url ?? ''}`, { label: 'Open', run: () => void show() });
      else toast.success('Stub saved to the project', `${r.file} — served when the mock server starts`, { label: 'Open mocks', run: () => void show() });
    } catch (e) {
      toast.error('Could not create the stub', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      className="mocks-from-response"
      title={
        <span className="row-sm">
          <Server size={16} /> Mock this response
        </span>
      }
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!built.stub || busy} onClick={create}>
            {busy ? <Spinner /> : <Server />} Create stub
          </button>
        </>
      }
    >
      {built.error ? (
        <Callout kind="warning">{built.error}</Callout>
      ) : (
        <div className="grid-2" style={{ gridTemplateColumns: '1fr 1.3fr' }}>
          <div className="col">
            <Field label="URL matching">
              <Segmented
                value={o.urlMode}
                onChange={(urlMode) => setO({ ...o, urlMode })}
                options={[
                  { value: 'urlPath', label: 'Exact path' },
                  { value: 'url', label: 'Path + query' },
                  { value: 'urlPathPattern', label: 'Generalise ids' },
                ]}
              />
            </Field>
            {o.urlMode !== 'url' && <Checkbox checked={o.matchQuery} onChange={(matchQuery) => setO({ ...o, matchQuery })} label="Match query parameters" />}
            {!!result.sent?.body && <Checkbox checked={o.matchBody} onChange={(matchBody) => setO({ ...o, matchBody })} label="Match the request body" />}
            <Field label="Folder" hint="Shown in the mock server's stub tree">
              <input className="input" value={o.folder} onChange={(e) => setO({ ...o, folder: e.target.value })} placeholder="(root)" />
            </Field>
            <Checkbox
              checked={o.randomize}
              onChange={(randomize) => setO({ ...o, randomize })}
              label="Randomize the data (same shape, fresh values per call)"
              title={isJson ? undefined : 'Only JSON bodies can be randomized'}
            />
            {o.randomize && <Checkbox checked={o.keepIds} onChange={(keepIds) => setO({ ...o, keepIds })} label="Keep ids stable" />}
            {o.randomize && !isTauri && <div className="text-warning xs">Randomizing needs the desktop app.</div>}
            <div className="subtle xs">Saved as a file in mocks/mappings/ — commit it with your code. Served by the project's mock server.</div>
          </div>
          <div className="editor-shell">
            <CodeEditor value={built.stub ? prettyJson(built.stub) : ''} readOnly language="json" />
          </div>
        </div>
      )}
    </Modal>
  );
}
