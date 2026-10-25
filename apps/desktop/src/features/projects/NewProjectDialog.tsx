/**
 * Creating (or opening) a project in a folder without `devall.yaml`. When the folder holds
 * several git repositories — say `frontend/web`, `frontend/admin`, `backend/api` — they are
 * offered as the repositories of a multi-repo project (saved to `devall.yaml` with relative paths
 * and a group each). A folder that is, or is inside, one repo becomes a project as before.
 */
import { useState } from 'react';
import { create } from 'zustand';
import { FolderPlus, GitBranch } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { reposApi, type Repo } from '@/lib/api/git';
import { suggestedGroup } from '@/features/git/model';
import { useRepos } from '@/store/repos';
import { useWorkspace } from '@/store/workspace';

interface Target {
  dir: string;
  repos: Repo[];
  /** The user tried to open the folder (it has no devall.yaml) rather than create a project. */
  opening: boolean;
}

export const useNewProject = create<{ target: Target | null; show(t: Target): void; close(): void }>((set) => ({
  target: null,
  show: (target) => set({ target }),
  close: () => set({ target: null }),
}));

/**
 * Opens `dir` when it is a project; otherwise offers to create one there, listing the git
 * repositories found in it.
 */
export async function openOrCreateProject(dir: string, opening: boolean) {
  try {
    const d = await reposApi.detect(dir);
    if (d.isProject) {
      await useWorkspace.getState().openProject(dir);
      return;
    }
    useNewProject.getState().show({ dir, repos: d.repos, opening });
  } catch (e) {
    toast.error('Could not read the folder', errorMessage(e));
  }
}

export function NewProjectHost() {
  const target = useNewProject((s) => s.target);
  return target ? <NewProjectDialog key={target.dir} target={target} /> : null;
}

function NewProjectDialog({ target }: { target: Target }) {
  const close = useNewProject((s) => s.close);
  const nested = target.repos.filter((r) => r.origin === 'discovered');
  const containing = target.repos.find((r) => r.origin === 'containing');
  const [name, setName] = useState(() => target.dir.split(/[\\/]/).filter(Boolean).pop() ?? 'My project');
  const [picked, setPicked] = useState<Record<string, boolean>>(() => Object.fromEntries(nested.map((r) => [r.name, true])));
  const [groups, setGroups] = useState<Record<string, string>>(() => Object.fromEntries(nested.map((r) => [r.name, suggestedGroup(r.relPath)])));
  const [busy, setBusy] = useState(false);
  const chosen = nested.filter((r) => picked[r.name]);

  const createIt = async () => {
    setBusy(true);
    try {
      if (!(await useWorkspace.getState().createProject(target.dir, name.trim()))) return;
      const root = useWorkspace.getState().project?.root;
      if (root && chosen.length) {
        const g = Object.fromEntries(chosen.map((r) => [r.name, groups[r.name] ?? '']).filter(([, v]) => v.trim()));
        await reposApi.saveDiscovered(root, chosen.map((r) => r.name), g);
        toast.success(`${chosen.length} ${chosen.length === 1 ? 'repository' : 'repositories'} added`, 'Listed in devall.yaml — commit it to share the project with your team.');
      }
      await useRepos.getState().refreshOverview().catch(() => {});
      close();
    } catch (e) {
      toast.error('Could not add the repositories', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={close}
      size="lg"
      title={
        <span className="row-sm">
          <FolderPlus size={16} /> {target.opening ? 'Create a project here?' : 'New project'}
        </span>
      }
      footer={
        <>
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || !name.trim()} onClick={createIt}>
            {busy ? <Spinner /> : <FolderPlus />} {chosen.length ? `Create multi-repo project (${chosen.length})` : 'Create project'}
          </button>
        </>
      }
    >
      <div className="col">
        <div className="mono xs truncate subtle" title={target.dir}>
          {target.dir}
        </div>
        {target.opening && <p className="small muted">There's no devall.yaml in this folder yet. Create a project here to keep requests, environments, mocks and notes next to your code.</p>}
        <Field label="Project name">
          <input className="input" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        {containing && (
          <Callout>
            This folder is {containing.projectPrefix ? 'inside' : ''} the git repository <strong>{containing.name}</strong>
            {containing.projectPrefix ? ` (at ${containing.projectPrefix})` : ''} — Source control uses it automatically.
          </Callout>
        )}
        {nested.length > 0 && (
          <div className="col-sm">
            <span className="small strong row-sm">
              <GitBranch size={14} /> Found {nested.length} git {nested.length === 1 ? 'repository' : 'repositories'} in this folder
            </span>
            <span className="xs subtle">Selected ones are listed in devall.yaml (relative paths) so they share this project's requests and environments, and appear in Source control. Groups organise them (e.g. Frontend, Backend).</span>
            <div className="git-table-wrap">
              <table className="table">
                <tbody>
                  {nested.map((r) => (
                    <tr key={r.name}>
                      <td style={{ width: 28 }}>
                        <Checkbox checked={!!picked[r.name]} onChange={(v) => setPicked((p) => ({ ...p, [r.name]: v }))} />
                      </td>
                      <td>
                        <div className="strong small">{r.name}</div>
                        <div className="mono xs subtle">{r.relPath}</div>
                      </td>
                      <td style={{ width: 180 }}>
                        <input className="input sm" placeholder="Group" value={groups[r.name] ?? ''} disabled={!picked[r.name]} onChange={(e) => setGroups((g) => ({ ...g, [r.name]: e.target.value }))} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}
        <p className="xs subtle">Repositories elsewhere on disk (e.g. next to this folder) can be added later: project menu → Repositories…</p>
      </div>
    </Modal>
  );
}
