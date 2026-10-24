/**
 * Command-palette entries for the mock server module. The palette can map them to its `Cmd`
 * shape, e.g. `MOCK_PALETTE_ACTIONS.map((a) => ({ id: a.id, group: 'Mock server', label: a.label, text: a.keywords, run: a.run }))`.
 */
import { useNav as useAppNav } from '@/store/nav';
import { useNav, type Section } from './store/nav';
import { useMockTarget } from './store/target';
import { BLANK_STUB, openNewStub } from './stubs/open';

export interface MockPaletteAction {
  id: string;
  label: string;
  /** Extra search words. */
  keywords: string;
  run(): void;
}

function goTo(section: Section, param?: string) {
  useAppNav.getState().go('mocks');
  useNav.getState().go(section, param);
}

export const MOCK_PALETTE_ACTIONS: MockPaletteAction[] = [
  { id: 'mocks.stubs', label: 'Mock server: stubs', keywords: 'mock wiremock stubs mappings', run: () => goTo('stubs') },
  {
    id: 'mocks.new-stub',
    label: 'Mock server: new stub',
    keywords: 'mock wiremock create stub mapping',
    run: () => {
      useAppNav.getState().go('mocks');
      openNewStub(structuredClone(BLANK_STUB));
    },
  },
  { id: 'mocks.requests', label: 'Mock server: request journal', keywords: 'mock wiremock requests journal traffic', run: () => goTo('requests') },
  { id: 'mocks.near-misses', label: 'Mock server: near misses', keywords: 'mock wiremock unmatched near miss', run: () => goTo('near-misses') },
  { id: 'mocks.record', label: 'Mock server: record & snapshot', keywords: 'mock wiremock record proxy snapshot', run: () => goTo('recording') },
  { id: 'mocks.import', label: 'Mock server: import / export stubs', keywords: 'mock wiremock import export har openapi postman zip', run: () => goTo('import') },
  { id: 'mocks.compare', label: 'Mock server: compare servers', keywords: 'mock wiremock compare diff remote copy', run: () => goTo('compare') },
  { id: 'mocks.drift', label: 'Mock server: drift against the OpenAPI spec', keywords: 'mock openapi drift contract spec schema validate', run: () => goTo('drift') },
  { id: 'mocks.servers', label: 'Mock server: remote servers', keywords: 'mock wiremock remote server connect', run: () => goTo('servers') },
  {
    id: 'mocks.start',
    label: 'Mock server: start',
    keywords: 'mock wiremock start run serve',
    run: () => {
      useAppNav.getState().go('mocks');
      const st = useMockTarget.getState();
      void (st.root ? st.start() : Promise.resolve());
    },
  },
  {
    id: 'mocks.stop',
    label: 'Mock server: stop',
    keywords: 'mock wiremock stop',
    run: () => void useMockTarget.getState().stop(),
  },
];
