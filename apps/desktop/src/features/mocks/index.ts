// Mock server module. The page is the default export of ./MocksPage (lazy-load it from App.tsx).
// Pieces other modules can use without loading the page:
export { MockFromResponseButton } from './MockFromResponseButton';
export { createStubFromResponse, stubFromSendResult, saveStubToProject, DEFAULT_FROM_RESPONSE, type FromResponseOptions, type CreatedStub } from './fromResponse';
export { MOCK_PALETTE_ACTIONS, type MockPaletteAction } from './palette';
export { requestForStub } from './stubs/openInRequests';
export { openStubFile } from './stubs/openFile';
