import { useEffect, useState } from 'react';
import { FolderOpen, FolderPlus, NotebookPen, X, GitBranch } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { RelTime } from '@/components/ui/misc';
import { promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { RecentProject } from '@/lib/types';
import { useWorkspace } from '@/store/workspace';

export default function ProjectsPage() {
  const ws = useWorkspace();
  const [recent, setRecent] = useState<RecentProject[]>([]);
  useEffect(() => {
    api.recentProjects().then(setRecent).catch(() => {});
  }, [ws.project?.root]);

  const openFolder = async () => {
    const dir = await openDialog({ directory: true, title: 'Open a /dev/all project (folder with devall.yaml)' });
    if (typeof dir !== 'string') return;
    if (await ws.openProject(dir)) return;
  };

  const create = async () => {
    const dir = await openDialog({ directory: true, title: 'Choose a folder for the new project (e.g. your service’s git repo)' });
    if (typeof dir !== 'string') return;
    const suggested = dir.split(/[\\/]/).filter(Boolean).pop() ?? 'My project';
    const name = await promptDialog({ title: 'New project', label: 'Project name', initial: suggested, validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    await ws.createProject(dir, name);
  };

  return (
    <div className="page">
      <div className="page-header">
        <h1>Projects</h1>
      </div>
      <div className="page-body narrow col-lg">
        <p className="muted">
          A project is a folder with a <code className="code-inline">devall.yaml</code> — usually your service’s git repo. Requests, environments and (soon) mocks are plain YAML files inside it, so you review and share them with git.
          Secrets never go in those files.
        </p>
        <div className="grid-3">
          <button className="card hoverable project-card" onClick={create}>
            <FolderPlus />
            <span className="strong">New project</span>
            <span className="subtle xs">Initialize a folder or repo</span>
          </button>
          <button className="card hoverable project-card" onClick={openFolder}>
            <FolderOpen />
            <span className="strong">Open project</span>
            <span className="subtle xs">A folder containing devall.yaml</span>
          </button>
          <button className="card hoverable project-card" onClick={() => ws.openScratch().catch((e) => toast.error('Could not open scratch pad', errorMessage(e)))}>
            <NotebookPen />
            <span className="strong">Scratch pad</span>
            <span className="subtle xs">Personal requests, no folder needed</span>
          </button>
        </div>
        {recent.length > 0 && (
          <section className="section">
            <div className="section-head">
              <h3>Recent</h3>
            </div>
            <div className="panel">
              {recent.map((r) => (
                <div key={r.path} className="list-item" onClick={() => ws.openProject(r.path)}>
                  <GitBranch size={14} className="subtle" />
                  <span className="strong">{r.name}</span>
                  <span className="subtle xs mono truncate grow">{r.path}</span>
                  <span className="subtle xs">
                    <RelTime ts={r.openedAt} />
                  </span>
                  <button
                    className="icon-btn sm"
                    title="Remove from list"
                    onClick={async (e) => {
                      e.stopPropagation();
                      await api.forgetRecent(r.path);
                      setRecent((x) => x.filter((y) => y.path !== r.path));
                    }}
                  >
                    <X />
                  </button>
                </div>
              ))}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
