/**
 * Registers every link provider and wires navigation/messages — imported once by the app shell.
 * Each provider lives with its feature (`features/<name>/linkProvider.tsx`); pages and repositories,
 * which have no feature of their own here, are in `./core.tsx`.
 */
import { toast } from '@/components/ui/toast';
import { requestLinks } from '@/features/http/linkProvider';
import { noteLinks } from '@/features/notes/linkProvider';
import { todoLinks } from '@/features/todos/linkProvider';
import { envLinks } from '@/features/environments/linkProvider';
import { stubLinks } from '@/features/mocks/linkProvider';
import { loadLinks } from '@/features/load/linkProvider';
import { fakeDataLinks } from '@/features/data/linkProvider';
import { dbLinks } from '@/features/db/linkProvider';
import { webhookLinks } from '@/features/webhooks/linkProvider';
import { utilityLinks } from '@/features/utilities/linkProvider';
import { gitLinks } from '@/features/git/linkProvider';
import { useWorkbench } from '@/store/workbench';
import { pageLinks, repoLinks } from './core';
import { configureLinks, registerLinkProvider } from './registry';

let done = false;

export function registerLinkProviders() {
  if (done) return;
  done = true;
  configureLinks({
    // Outside any view (palette, top bar): the active group, like the sidebar.
    go: (section, param, opts) => useWorkbench.getState().open(section, param, opts ?? {}),
    info: (title, detail) => toast.info(title, detail),
  });
  for (const p of [requestLinks, noteLinks, todoLinks, envLinks, stubLinks, loadLinks, fakeDataLinks, dbLinks, webhookLinks, utilityLinks, repoLinks, gitLinks, pageLinks]) registerLinkProvider(p);
}
