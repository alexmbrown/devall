import { useEffect, useState } from 'react';
import { FolderOpen, FolderPlus, NotebookPen, X, GitBranch, Settings2 } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { RelTime } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { openReposDialog } from '@/features/git/dialogs';
import { openOrCreateProject } from './NewProjectDialog';
import { api, errorMessage } from '@/lib/ipc';
import type { RecentProject } from '@/lib/types';
import { useWorkspace } from '@/store/workspace';

export default function ProjectsPage() {
  const ws = useWorkspace();
  const [recent, setRecent] = useState<RecentProject[]>([]);
  useEffect(() => {
    api.recentProjects().then(setRecent).catch(() => {});
  }, [ws.project?.root]);

  // A folder without devall.yaml offers to become a project — a multi-repo one when it holds
  // several git repositories.
  const openFolder = async () => {
    const dir = await openDialog({ directory: true, title: 'Open a /dev/all project (folder with devall.yaml)' });
    if (typeof dir === 'string') await openOrCreateProject(dir, true);
  };

  const create = async () => {
    const dir = await openDialog({ directory: true, title: 'Choose a folder for the new project (your service’s git repo, or a folder holding several repos)' });
    if (typeof dir === 'string') await openOrCreateProject(dir, false);
  };

  return (
    <div className="page">
      <div className="page-header">
        <h1>Projects</h1>
      </div>
      <div className="page-body narrow col-lg">
        <p className="muted">
          A project is a folder with a <code className="code-inline">devall.yaml</code> — usually your service’s git repo, or a folder above several repos (frontends and backends that share requests, environments and
          mocks). Everything in it is plain files, so you review and share them with git. Secrets never go in those files.
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
        {ws.project && !ws.project.isScratch && (
          <section className="section">
            <div className="section-head">
              <h3>{ws.project.manifest.name}</h3>
            </div>
            <div className="row-sm wrap">
              <button className="btn btn-sm" onClick={openReposDialog}>
                <Settings2 /> Repositories…
              </button>
              <span className="xs subtle">Add the repositories this project spans, locate or clone missing ones, group them.</span>
            </div>
          </section>
        )}
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
