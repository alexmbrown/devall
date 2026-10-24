import { api } from '@/lib/ipc';
import { useNav as useAppNav } from '@/store/nav';
import type { StubMapping } from '@/features/mocks/lib/types';
import { stubId } from '@/features/mocks/lib/stub-utils';

/**
 * Opens the Mock server page on the stub defined in `mocks/<file>` (e.g. `mappings/orders.json`),
 * used by note links. The stubs page resolves the file against the loaded mappings.
 */
export function openStubFile(file: string) {
  // The nonce makes a repeated click on the same link navigate again.
  useAppNav.getState().go('mocks', `stubs/file:${file}|${Date.now()}`);
}

/** `file:<path>|<nonce>` deep-link param → the path under `mocks/`. */
export function stubFileParam(param: string | undefined): string | null {
  if (!param?.startsWith('file:')) return null;
  return param.slice(5).split('|')[0] || null;
}

const canon = (v: unknown) => JSON.stringify(v ?? null);

/**
 * The loaded stub defined by a mapping file: by id when the file has one, otherwise the first
 * entry whose request (and name) match — the engine gives id-less stubs a stable generated id.
 */
export async function findStubForFile(root: string, file: string, mappings: StubMapping[]): Promise<StubMapping | null> {
  const text = await api.readTextFile(`${root}/mocks/${file}`);
  const json = JSON.parse(text) as StubMapping | { mappings?: StubMapping[] };
  const entries: StubMapping[] = 'mappings' in json && Array.isArray(json.mappings) ? json.mappings : [json as StubMapping];
  for (const e of entries) {
    const id = e.id ?? e.uuid;
    const hit = id ? mappings.find((m) => stubId(m) === id) : mappings.find((m) => canon(m.request) === canon(e.request) && (m.name ?? '') === (e.name ?? ''));
    if (hit) return hit;
  }
  return null;
}
