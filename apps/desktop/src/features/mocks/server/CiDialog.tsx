import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Container } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { CopyButton, Field, Segmented } from '@/features/mocks/components/ui';
import { loadApi, type LoadTestEntry } from '@/lib/api/load';
import { useWorkspace } from '@/store/workspace';
import { builtinPrefs } from '@/features/mocks/store/target';
import { cliCommands, compose, dockerCommands, githubActions, type CiOptions } from './ciSnippets';

type Tab = 'cli' | 'docker' | 'github' | 'compose';

const PREFS_KEY = 'devall.mocks.ci';

function readPrefs(): Partial<CiOptions> {
  try {
    return JSON.parse(localStorage.getItem(PREFS_KEY) ?? '{}');
  } catch {
    return {};
  }
}

function Code({ text }: { text: string }) {
  return (
    <div className="ci-code">
      <pre className="code">{text}</pre>
      <span className="copy">
        <CopyButton text={text} title="Copy" />
      </span>
    </div>
  );
}

/** "Run in CI": the exact commands to serve this project's mocks with the CLI, Docker, GitHub Actions or compose. */
export function CiDialog({ onClose }: { onClose(): void }) {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const [tab, setTab] = useState<Tab>('docker');
  const [shell, setShell] = useState<'bash' | 'powershell'>(navigator.platform.startsWith('Win') ? 'powershell' : 'bash');
  const [opts, setOpts] = useState<CiOptions>(() => ({
    image: 'devall-mock',
    port: builtinPrefs(root).port ?? 8080,
    chaos: '',
    projectDir: '.',
    ...readPrefs(),
  }));
  const [tests, setTests] = useState<LoadTestEntry[]>([]);
  useEffect(() => {
    if (root) loadApi.list(root).then(setTests).catch(() => setTests([]));
  }, [root]);
  useEffect(() => {
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify({ image: opts.image, chaos: opts.chaos, projectDir: opts.projectDir }));
    } catch {
      /* storage unavailable */
    }
  }, [opts.image, opts.chaos, opts.projectDir]);
  const set = (p: Partial<CiOptions>) => setOpts((o) => ({ ...o, ...p }));
  const withTest = useMemo(() => ({ ...opts, loadTest: tests.find((t) => !t.error)?.path }), [opts, tests]);
  const docker = dockerCommands(opts);

  return (
    <Modal
      open
      onClose={onClose}
      size="xl"
      title={
        <span className="row-sm">
          <Container size={16} /> Run these mocks in CI
        </span>
      }
    >
      <div className="col ci-tabs">
        <p className="subtle small">
          The same stubs, served by the same engine outside the app: <code className="code-inline">devall mock serve</code> or the Docker image built from the /dev/all repository
          (<code className="code-inline">docker build -t devall-mock .</code> at its root; published as <code className="code-inline">ghcr.io/&lt;owner&gt;/devall-mock</code> on release tags). The
          admin API has no authentication — only expose the port on CI or a trusted network. Details: <code className="code-inline">docs/MOCKS_IN_CI.md</code>.
        </p>
        <div className="grid-2">
          <Field label="Image">
            <input className="input mono" value={opts.image} onChange={(e) => set({ image: e.target.value.trim() || 'devall-mock' })} />
          </Field>
          <Field label="Project folder in the repository" hint="`.` when devall.yaml is at the repository root">
            <input className="input mono" value={opts.projectDir} onChange={(e) => set({ projectDir: e.target.value })} />
          </Field>
          <Field label="Host port">
            <input className="input" type="number" min={1} max={65535} value={opts.port} onChange={(e) => set({ port: Math.min(65535, Math.max(1, Number(e.target.value) || 8080)) })} />
          </Field>
          <Field label="Chaos">
            <select className="select" value={opts.chaos} onChange={(e) => set({ chaos: e.target.value })}>
              <option value="">Off</option>
              <option value="slow">Slow network (+800 ms ± 600)</option>
              <option value="flaky">Flaky backend (10% 5xx)</option>
              <option value="broken">Broken connections (5% faults)</option>
              <option value="all">Everything at once</option>
            </select>
          </Field>
        </div>
        <div className="subtabs" role="tablist">
          {(
            [
              ['docker', 'Docker'],
              ['github', 'GitHub Actions'],
              ['compose', 'docker-compose'],
              ['cli', 'CLI'],
            ] as [Tab, string][]
          ).map(([id, label]) => (
            <button key={id} role="tab" aria-selected={tab === id} className={clsx('subtab', tab === id && 'on')} onClick={() => setTab(id)}>
              {label}
            </button>
          ))}
          <span className="spacer" />
          {tab === 'docker' && (
            <Segmented
              value={shell}
              onChange={setShell}
              options={[
                { value: 'bash', label: 'bash / zsh' },
                { value: 'powershell', label: 'PowerShell' },
              ]}
            />
          )}
        </div>
        {tab === 'docker' && <Code text={docker[shell]} />}
        {tab === 'github' && <Code text={githubActions(opts)} />}
        {tab === 'compose' && <Code text={compose(opts)} />}
        {tab === 'cli' && <Code text={cliCommands(withTest)} />}
        <p className="subtle xs">
          Run from the repository root. Stubs are read when the server starts; after editing files call <code className="code-inline">POST /__admin/mappings/reset</code> or restart
          the container. Requests are logged to stdout (<code className="code-inline">--log json</code> for JSON lines).
        </p>
      </div>
    </Modal>
  );
}
