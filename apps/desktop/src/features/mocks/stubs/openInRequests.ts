import { useWorkspace } from '@/store/workspace';
import { toast } from '@/components/ui/toast';
import type { Body, HttpRequestFile, KeyValue } from '@/lib/types';
import type { StubMapping } from '@/features/mocks/lib/types';
import { exampleRequest } from '@/features/mocks/lib/codegen';
import { displayName } from '@/features/mocks/lib/stub-utils';

/** An HTTP-client request that calls `stub` on the mock server at `baseUrl`. */
export function requestForStub(stub: StubMapping, baseUrl: string): HttpRequestFile {
  const ex = exampleRequest(stub.request ?? {});
  const ctKey = Object.keys(ex.headers).find((h) => h.toLowerCase() === 'content-type');
  const ct = ctKey ? ex.headers[ctKey] : '';
  let body: Body | undefined;
  if (ex.body !== undefined) {
    if (ct.includes('json')) body = { type: 'json', content: ex.body };
    else if (ct.includes('xml')) body = { type: 'xml', content: ex.body };
    else if (ct.includes('x-www-form-urlencoded')) {
      body = { type: 'form', fields: [...new URLSearchParams(ex.body)].map(([name, value]) => ({ name, value })) };
    } else body = { type: 'text', content: ex.body, contentType: ct || undefined };
  }
  // The body type sets Content-Type itself (except free text, which carries it above).
  const headers: KeyValue[] = Object.entries(ex.headers)
    .filter(([k]) => !(body && k === ctKey))
    .map(([name, value]) => ({ name, value }));
  const method = ex.method.toUpperCase() === 'ANY' ? 'GET' : ex.method.toUpperCase();
  return {
    id: '',
    name: stub.name || displayName(stub),
    method,
    url: baseUrl.replace(/\/+$/, '') + ex.path,
    ...(headers.length ? { headers } : {}),
    ...(body ? { body } : {}),
  };
}

/** "Open in Requests": creates a request in the project's http/ folder and switches to it. */
export async function openInRequests(stub: StubMapping, baseUrl: string) {
  if (!baseUrl) return toast.error('The mock server is not running', 'Start it first so the request has a URL to call.');
  if (!useWorkspace.getState().project) return toast.error('Open a project first');
  await useWorkspace.getState().newRequest('', requestForStub(stub, baseUrl));
}
