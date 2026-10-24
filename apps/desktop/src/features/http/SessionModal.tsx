import { useEffect, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { create } from 'zustand';
import { Modal } from '@/components/ui/Modal';
import { Empty, Segmented } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { CookieInfo } from '@/lib/types';
import { useVarsVersion } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';

export const useSessionModal = create<{ open: boolean; setOpen(v: boolean): void }>((set) => ({ open: false, setOpen: (open) => set({ open }) }));

/** Cookie jar and captured runtime variables for the open project (this app session). */
export function SessionModal() {
  const open = useSessionModal((s) => s.open);
  const setOpen = useSessionModal((s) => s.setOpen);
  const project = useWorkspace((s) => s.project);
  const [tab, setTab] = useState<'vars' | 'cookies'>('vars');
  const [cookies, setCookies] = useState<CookieInfo[]>([]);
  const [vars, setVars] = useState<Record<string, string>>({});

  const load = () => {
    if (!project) return;
    api.cookiesList(project.root).then(setCookies).catch(() => setCookies([]));
    api.runtimeVars(project.root).then(setVars).catch(() => setVars({}));
  };
  useEffect(() => {
    if (open) load();
  }, [open, project?.root]);

  if (!open || !project) return null;
  const setRuntime = async (next: Record<string, string>) => {
    try {
      await api.setRuntimeVars(project.root, next);
      setVars(next);
      useVarsVersion.getState().bump();
    } catch (e) {
      toast.error('Could not update variables', errorMessage(e));
    }
  };

  return (
    <Modal
      open
      onClose={() => setOpen(false)}
      title="Session"
      size="lg"
      headerExtra={
        <Segmented
          value={tab}
          onChange={setTab}
          options={[
            { value: 'vars', label: `Captured variables (${Object.keys(vars).length})` },
            { value: 'cookies', label: `Cookies (${cookies.length})` },
          ]}
        />
      }
    >
      {tab === 'vars' ? (
        Object.keys(vars).length ? (
          <div className="col">
            <p className="muted small">Values captured from responses (Tests tab → Captures). They override environment variables until the app quits.</p>
            <table className="table">
              <tbody>
                {Object.entries(vars).map(([k, v]) => (
                  <tr key={k}>
                    <td className="mono xs strong">{`{{${k}}}`}</td>
                    <td className="mono xs break">{v.length > 300 ? `${v.slice(0, 300)}…` : v}</td>
                    <td style={{ width: 32 }}>
                      <button
                        className="icon-btn sm danger"
                        onClick={() => {
                          const { [k]: _gone, ...rest } = vars;
                          setRuntime(rest);
                        }}
                        aria-label={`Remove ${k}`}
                      >
                        <Trash2 />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="row">
              <button className="btn btn-sm btn-danger-ghost" onClick={() => setRuntime({})}>
                Clear all
              </button>
            </div>
          </div>
        ) : (
          <Empty title="No captured variables">Add captures in a request’s Tests tab, e.g. save $.access_token as {'{{token}}'}.</Empty>
        )
      ) : cookies.length ? (
        <div className="col">
          <table className="table">
            <thead>
              <tr>
                <th>Domain</th>
                <th>Name</th>
                <th>Value</th>
                <th>Flags</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {cookies.map((c) => (
                <tr key={`${c.domain}|${c.path}|${c.name}`}>
                  <td className="mono xs">
                    {c.domain}
                    {c.path !== '/' && c.path}
                  </td>
                  <td className="mono xs strong">{c.name}</td>
                  <td className="mono xs break">{c.value.length > 80 ? `${c.value.slice(0, 80)}…` : c.value}</td>
                  <td className="xs subtle nowrap">
                    {[c.secure && 'Secure', c.httpOnly && 'HttpOnly', c.expires ? `expires ${new Date(c.expires).toLocaleString()}` : 'session'].filter(Boolean).join(' · ')}
                  </td>
                  <td>
                    <button
                      className="icon-btn sm danger"
                      onClick={async () => {
                        await api.cookiesDelete(project.root, c.domain, c.path, c.name);
                        load();
                      }}
                      aria-label="Delete cookie"
                    >
                      <Trash2 />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="row">
            <button
              className="btn btn-sm btn-danger-ghost"
              onClick={async () => {
                await api.cookiesClear(project.root);
                load();
              }}
            >
              Clear cookie jar
            </button>
            <span className="subtle xs">Each project has its own jar; nothing is persisted after the app quits.</span>
          </div>
        </div>
      ) : (
        <Empty title="No cookies yet">Cookies set by responses are stored here and sent with matching requests.</Empty>
      )}
    </Modal>
  );
}
