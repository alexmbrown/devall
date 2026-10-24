import { useStubTabs } from '@/features/mocks/store/tabs';
import { useNav } from '@/features/mocks/store/nav';
import { activeTargetOf, serverKey } from '@/features/mocks/store/target';
import type { StubMapping } from '@/features/mocks/lib/types';
import { displayName, stubId } from '@/features/mocks/lib/stub-utils';

export function activeSid(): string {
  return serverKey(activeTargetOf());
}

export function openStub(stub: StubMapping, opts: { preview?: boolean; navigate?: boolean } = {}) {
  const sid = activeSid();
  useStubTabs.getState().open(sid, { stubId: stubId(stub), title: displayName(stub), method: stub.request?.method }, { preview: opts.preview });
  if (opts.navigate !== false) useNav.getState().go('stubs');
}

export function openNewStub(draft: StubMapping, title = 'New stub') {
  const sid = activeSid();
  useStubTabs.getState().open(sid, { isNew: true, title, method: draft.request?.method, draft });
  useNav.getState().go('stubs');
}

export const BLANK_STUB: StubMapping = {
  name: '',
  request: { method: 'GET', urlPath: '/api/example' },
  response: { status: 200, headers: { 'Content-Type': 'application/json' }, jsonBody: { message: 'Hello from the mock server' } },
};
