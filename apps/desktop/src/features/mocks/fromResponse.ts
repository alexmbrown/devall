/**
 * "HTTP request → mock": turn a response from the HTTP client into a stub of the project's
 * built-in mock server. With the server running the stub goes through its admin API (and is
 * written to `mocks/mappings/` because it is persistent); otherwise the mapping file is written
 * directly and loaded on the next start.
 */
import { api, errorMessage } from '@/lib/ipc';
import { mockApi } from '@/lib/api/mock';
import { dataApi } from '@/lib/api/data';
import type { SendResult } from '@/lib/types';
import type { LoggedRequest, LoggedResponse, StubMapping } from './lib/types';
import { requestToStub, DEFAULT_TO_STUB, type ToStubOptions } from './lib/request-to-stub';
import { withFolder, withWmui, prettyJson } from './lib/stub-utils';
import { WireMockClient } from './lib/api';
import { joinPath } from './lib/servers';
import { uid } from '@/lib/id';
import { useMockTarget } from './store/target';

export interface FromResponseOptions {
  urlMode: ToStubOptions['urlMode'];
  matchQuery: boolean;
  /** Match the request body (JSON → equalToJson with extra fields allowed). */
  matchBody: boolean;
  /** Folder (stub metadata, shown in the stub tree). */
  folder: string;
  /** Replace the JSON body's values with random-data helpers (needs the desktop app). */
  randomize: boolean;
  keepIds: boolean;
}

export const DEFAULT_FROM_RESPONSE: FromResponseOptions = { urlMode: 'urlPath', matchQuery: true, matchBody: false, folder: 'from-http', randomize: false, keepIds: true };

function headerRecord(pairs: [string, string][] | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of pairs ?? []) {
    const existing = Object.keys(out).find((x) => x.toLowerCase() === k.toLowerCase());
    if (existing) out[existing] = `${out[existing]}, ${v}`;
    else out[k] = v;
  }
  return out;
}

/** The stub (not yet saved) for a send result, or an error message when there is nothing to mock. */
export function stubFromSendResult(result: SendResult, o: FromResponseOptions = DEFAULT_FROM_RESPONSE): StubMapping {
  if (!result.sent) throw new Error('This request was not sent, so there is nothing to mock.');
  if (!result.response) throw new Error('There is no response to mock (the request failed).');
  let pathAndQuery = '/';
  try {
    const u = new URL(result.sent.url);
    pathAndQuery = u.pathname + u.search;
  } catch {
    pathAndQuery = result.sent.url.replace(/^[a-z]+:\/\/[^/]+/i, '') || '/';
  }
  const req: LoggedRequest = { method: result.sent.method, url: pathAndQuery, headers: headerRecord(result.sent.headers), body: result.sent.body ?? undefined };
  const r = result.response;
  const resp: LoggedResponse = {
    status: r.status,
    headers: headerRecord(r.headers),
    body: r.bodyText ?? undefined,
    bodyAsBase64: r.bodyText == null ? (r.bodyBase64 ?? undefined) : undefined,
  };
  const stub = requestToStub(req, resp, {
    ...DEFAULT_TO_STUB,
    urlMode: o.urlMode,
    matchQuery: o.matchQuery,
    body: o.matchBody ? 'contains-json-keys' : 'none',
    useResponse: true,
    headers: [],
  });
  const withMeta = withWmui(o.folder.trim() ? withFolder(stub, o.folder) : stub, { tags: ['from-http'] });
  return { ...withMeta, persistent: true };
}

/** Turns a JSON body into a randomized response template (devall-data's template bridge). */
export async function randomizeStub(stub: StubMapping, keepIds: boolean): Promise<StubMapping> {
  const r = stub.response;
  let json: unknown = r.jsonBody;
  if (json === undefined && typeof r.body === 'string') {
    try {
      json = JSON.parse(r.body);
    } catch {
      return stub;
    }
  }
  if (json === undefined) return stub;
  const template = await dataApi.toTemplate(JSON.stringify(json), keepIds);
  const response = { ...r, body: template, transformers: [...new Set([...(r.transformers ?? []), 'response-template'])] };
  delete response.jsonBody;
  return { ...stub, response };
}

export interface CreatedStub {
  stub: StubMapping;
  /** Served right away by the running mock server. */
  live: boolean;
  /** Base URL of the running mock server. */
  url?: string;
  /** Mapping file written directly (server not running), relative to the project. */
  file?: string;
}

/**
 * Creates a stub from an HTTP-client send result in the project's mock server.
 * Mount `<MockFromResponseButton>` for the UI; call this for programmatic use.
 */
export async function createStubFromResponse(root: string, result: SendResult, o: FromResponseOptions = DEFAULT_FROM_RESPONSE): Promise<CreatedStub> {
  let stub = stubFromSendResult(result, o);
  if (o.randomize) stub = await randomizeStub(stub, o.keepIds);
  return saveStubToProject(root, stub);
}

export async function saveStubToProject(root: string, stub: StubMapping): Promise<CreatedStub> {
  const status = await mockApi.status(root).catch(() => null);
  if (status?.running && status.url) {
    const st = useMockTarget.getState();
    if (st.root === root) st.markLocalWrite();
    const created = await new WireMockClient({ url: status.url, root }).createMapping({ ...stub, persistent: true });
    if (st.root === root) void st.refreshStatus();
    return { stub: created, live: true, url: status.url };
  }
  const id = stub.id ?? uid();
  const slug = (stub.name ?? 'stub').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60) || 'stub';
  const file = `mocks/mappings/${slug}-${id.slice(0, 8)}.json`;
  const { persistent: _p, ...onDisk } = { ...stub, id };
  void _p;
  try {
    await api.writeTextFile(joinPath(root, file), prettyJson(onDisk) + '\n');
  } catch (e) {
    throw new Error(`Could not write ${file}: ${errorMessage(e)}`);
  }
  return { stub: { ...stub, id }, live: false, file };
}
