import { useMemo, useState } from 'react';
import type { LoggedRequest, LoggedResponse } from '@/features/mocks/lib/types';
import { Modal } from '@/components/ui/Modal';
import { Checkbox, Field, Segmented } from '@/features/mocks/components/ui';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { DEFAULT_TO_STUB, requestToStub, type ToStubOptions } from '@/features/mocks/lib/request-to-stub';
import { openNewStub } from '@/features/mocks/stubs/open';
import { prettyJson } from '@/features/mocks/lib/stub-utils';

/** Options for turning a logged request into a stub, with live preview. */
export function CreateStubDialog({ request, response, onClose }: { request: LoggedRequest; response?: LoggedResponse; onClose(): void }) {
  const [o, setO] = useState<ToStubOptions>({ ...DEFAULT_TO_STUB, useResponse: !!response && response.status !== 404 });
  const headerNames = Object.keys(request.headers ?? {}).filter((h) => !/^(host|content-length|connection|accept-encoding|user-agent|cache-control|pragma|sec-|origin|referer)/i.test(h));
  const stub = useMemo(() => requestToStub(request, response, o), [request, response, o]);
  const hasBody = !!request.body;
  return (
    <Modal
      open
      onClose={onClose}
      title="Create stub from request"
      size="lg"
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => {
              openNewStub(stub, stub.name);
              onClose();
            }}
          >
            Open in editor
          </button>
        </>
      }
    >
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
          {headerNames.length > 0 && (
            <Field label="Match these headers">
              <div className="col-sm">
                {headerNames.map((h) => (
                  <Checkbox
                    key={h}
                    checked={o.headers.includes(h.toLowerCase())}
                    onChange={(v) => setO({ ...o, headers: v ? [...o.headers, h.toLowerCase()] : o.headers.filter((x) => x !== h.toLowerCase()) })}
                    label={<span className="mono xs">{h}</span>}
                  />
                ))}
              </div>
            </Field>
          )}
          {hasBody && (
            <Field label="Request body">
              <select className="select" value={o.body} onChange={(e) => setO({ ...o, body: e.target.value as ToStubOptions['body'] })}>
                <option value="equalToJson">Match JSON (equalToJson)</option>
                <option value="contains-json-keys">Match JSON, allow extra fields</option>
                <option value="equalTo">Match exact text</option>
                <option value="none">Don't match the body</option>
              </select>
            </Field>
          )}
          <Checkbox checked={o.useResponse} onChange={(useResponse) => setO({ ...o, useResponse })} label={response ? `Use the actual response (${response.status})` : 'No response recorded'} />
        </div>
        <div className="editor-shell" style={{ height: 420 }}>
          <CodeEditor value={prettyJson(stub)} readOnly language="json" />
        </div>
      </div>
    </Modal>
  );
}
