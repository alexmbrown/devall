// Lightweight entry for other modules (Docker/K8s logs, HTTP, mocks): opening something in the
// viewer doesn't load the viewer itself. The page is `./LogsPage` (default export, lazy-loaded).
export { openInLogViewer, useLogs, LOGS_SECTION, EVENTS_KEY, type LogViewerSource } from './store';
