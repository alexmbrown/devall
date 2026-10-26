/**
 * Following `devall:` links that come from outside the app — OS deep links from a browser, a chat,
 * an email or another editor (Rust: `src-tauri/src/modules/deep_link.rs`) — and in-app links that
 * name another project.
 *
 * Rust checks each OS link and queues it until the UI takes it (`deep_link_take`, at start-up and on
 * every `deep-link` event), so a cold start's link waits for the workspace. Here the link is checked
 * again (`checkIncomingLink`; refused → a toast, nothing else), its project resolved
 * (`resolveLinkProject`: the open project; a recent project with that `devall.yaml` id, opened like
 * the top bar's project switcher does; or a message offering "Open folder…" — never a path from the
 * link) and then followed by `openLink`. An outside link only navigates: the routes it opens are
 * held (`./hold.ts`), so pages don't connect, run or start anything until the user clicks.
 */
import { listen } from '@tauri-apps/api/event';
import { toast } from '@/components/ui/toast';
import { openProjectFolder } from '@/features/projects/recent';
import { api, isTauri } from '@/lib/ipc';
import type { IncomingLink } from '@/lib/types';
import { useWorkbench } from '@/store/workbench';
import { useWorkspace } from '@/store/workspace';
import { checkIncomingLink, isProjectScoped, resolveLinkProject } from './external';
import { holdRoute } from './hold';
import { openLink, type GoFn } from './registry';
import { formatLink, type ParsedLink } from './syntax';

let started = false;
let queue: Promise<void> = Promise.resolve();

/** Takes OS links now and whenever Rust says more arrived. Called once by the app shell. */
export function initIncomingLinks() {
  if (!isTauri || started) return;
  started = true;
  void listen('deep-link', () => void take()).catch(() => {});
  void take();
}

async function take() {
  let links: IncomingLink[];
  try {
    links = await api.deepLinkTake();
  } catch {
    return;
  }
  // One at a time, in arrival order (each may switch projects).
  for (const l of links) queue = queue.then(() => receive(l)).catch(() => {});
}

function workspaceReady(): Promise<void> {
  if (useWorkspace.getState().ready) return Promise.resolve();
  return new Promise((resolve) => {
    const stop = useWorkspace.subscribe((s) => {
      if (!s.ready) return;
      stop();
      resolve();
    });
  });
}

async function receive(incoming: IncomingLink) {
  await workspaceReady();
  const check = incoming.link ? checkIncomingLink(incoming.link) : null;
  if (!check?.ok) {
    toast.error('Can’t open this link', `${check ? check.reason : (incoming.error ?? 'not a valid link')}: ${incoming.raw}`);
    return;
  }
  await followLink(check.link, { external: true });
}

/** Navigation for an outside link: the active group (like the palette), each route held. */
const heldGo: GoFn = (section, param, opts) => {
  holdRoute(section, param);
  useWorkbench.getState().open(section, param, opts ?? {});
};

/** A link waiting for the user to open its project ("Open folder…"), followed when they do (for two minutes). */
let waiting: { link: ParsedLink; external: boolean; until: number } | null = null;

useWorkspace.subscribe((s, prev) => {
  const w = waiting;
  if (!w || !s.project || s.project.root === prev.project?.root) return;
  waiting = null;
  if (Date.now() < w.until && (!w.link.project || w.link.project === s.project.manifest.id)) void followLink(w.link, { external: w.external });
});

function waitForProject(link: ParsedLink, external: boolean) {
  waiting = { link, external, until: Date.now() + 120_000 };
}

/**
 * Follows a checked link: in the right project (switching to it when it's another recent one),
 * then like a click on it. `external`: from outside the app, so the pages it opens are held.
 */
export async function followLink(link: ParsedLink, { external }: { external: boolean }) {
  const ws = useWorkspace.getState();
  const open = ws.project ? { id: ws.project.manifest.id } : null;
  const elsewhere = !!link.project && link.project !== open?.id && isProjectScoped(link);
  const candidates = elsewhere ? await api.recentProjectIds().catch(() => []) : [];
  const where = resolveLinkProject(link, open, candidates);
  const target = formatLink(link.kind, link.id);
  switch (where.action) {
    case 'ask':
      waitForProject(link, external);
      toast.info('Open a project to follow this link', target, { label: 'Open folder…', run: () => void openProjectFolder() });
      return;
    case 'missing':
      waitForProject(link, external);
      toast.info('This link is for a project that isn’t on this device’s recent list', `Open the project’s folder to follow ${target}`, { label: 'Open folder…', run: () => void openProjectFolder() });
      return;
    case 'switch': {
      // The same path as the top bar's project switcher.
      const opened = where.to.scratch ? await ws.openScratch().then(() => true, () => false) : await ws.openProject(where.to.path);
      if (!opened || useWorkspace.getState().project?.manifest.id !== link.project) return;
      toast.info(`Switched to ${where.to.name}`, 'to open a link');
      break;
    }
  }
  await openLink(target, external ? { go: heldGo } : {});
}
