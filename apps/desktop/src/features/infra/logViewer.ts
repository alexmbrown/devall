// "Open in log viewer" for container/pod logs, exec transcripts and Cloud Logging tails.
//
// The integrator connects this to the log viewer once, e.g. in App.tsx:
//
//   import { setLogViewerOpener } from '@/features/infra/logViewer';
//   import { openInLogViewer } from '@/features/logs';
//   setLogViewerOpener(({ name, text }) => openInLogViewer({ type: 'text', name, text }));
//
// Until then the stub copies the text to the clipboard and says so.

import { toast } from '@/components/ui/toast';

export interface LogViewerPayload {
  /** Source name shown in the viewer, e.g. `api (container)` or `orders/api-7d9f · api`. */
  name: string;
  /** Plain text, one entry per line (ANSI codes already stripped). */
  text: string;
}

type Opener = (payload: LogViewerPayload) => void;

const stub: Opener = ({ name, text }) => {
  navigator.clipboard.writeText(text).then(
    () => toast.info('Log viewer not connected yet', `Copied ${text.split('\n').length} lines of "${name}" to the clipboard instead.`),
    () => toast.error('Log viewer not connected yet'),
  );
};

let opener: Opener = stub;

/** Connects "Open in log viewer" to the real viewer (call once at startup). */
export function setLogViewerOpener(fn: Opener) {
  opener = fn;
}

/** Opens text in the log viewer (or the stub). */
export function openLogsInViewer(payload: LogViewerPayload) {
  opener(payload);
}

/** Hook form, for components. */
export function useOpenInLogViewer(): Opener {
  return openLogsInViewer;
}
