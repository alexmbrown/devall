import { useEffect, useMemo, useState } from 'react';
import { Cable, Container, Save } from 'lucide-react';
import { Callout, Checkbox, Field, Segmented } from '@/components/ui/misc';
import { Modal } from '@/components/ui/Modal';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { infraApi, type ContainerRow, type ForwardFile, type ForwardSource, type KubeContexts, type TargetKind } from '@/lib/api/infra';
import { useWorkspace } from '@/store/workspace';
import { startForward, useInfraUi } from './store';

const KUBE_KINDS: { value: TargetKind; label: string }[] = [
  { value: 'service', label: 'Service' },
  { value: 'pod', label: 'Pod' },
  { value: 'deployment', label: 'Deployment' },
  { value: 'statefulset', label: 'StatefulSet' },
  { value: 'daemonset', label: 'DaemonSet' },
  { value: 'replicaset', label: 'ReplicaSet' },
];

const DEFAULT_TEMPLATE = 'http://127.0.0.1:{port}';

/** Preview of the bound value (mirrors the Rust template rules for the common placeholders). */
function preview(template: string, spec: ForwardFile): string {
  const port = spec.localPort ? String(spec.localPort) : '<port>';
  return (template || DEFAULT_TEMPLATE)
    .replace(/\{\{[^}]*\}\}|\{(port|localPort|host|remotePort|name|namespace|target)\}/g, (m, key?: string) => {
      if (!key) return m;
      if (key === 'port' || key === 'localPort') return port;
      if (key === 'host') return '127.0.0.1';
      if (key === 'remotePort') return String(spec.remotePort || '');
      if (key === 'name') return spec.target.name;
      if (key === 'namespace') return spec.namespace ?? '';
      return `${spec.target.kind}/${spec.target.name}`;
    });
}

/** Create/edit/start a port-forward. Opened through `useInfraUi.openForward`. */
export function ForwardDialog() {
  const draft = useInfraUi((s) => s.draft);
  const close = useInfraUi((s) => s.closeForward);
  const project = useWorkspace((s) => s.project);
  const [spec, setSpec] = useState<ForwardFile | null>(null);
  const [bind, setBind] = useState(true);
  const [saveFile, setSaveFile] = useState(false);
  const [contexts, setContexts] = useState<KubeContexts | null>(null);
  const [namespaces, setNamespaces] = useState<string[]>([]);
  const [containers, setContainers] = useState<ContainerRow[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!draft) return;
    setSpec(structuredClone(draft.spec));
    setBind(!!draft.spec.bindEnvironment);
    setSaveFile(!!draft.id);
  }, [draft]);

  const source: ForwardSource = spec?.source ?? 'kubernetes';
  useEffect(() => {
    if (!draft) return;
    if (source === 'kubernetes') infraApi.kube.contexts().then(setContexts, () => setContexts(null));
    else infraApi.docker.containers(false).then(setContainers, () => setContainers([]));
  }, [draft, source]);

  const ctx = spec?.context ?? contexts?.current ?? '';
  useEffect(() => {
    if (!draft || source !== 'kubernetes' || !ctx) return;
    infraApi.kube.namespaces(ctx).then(setNamespaces, () => setNamespaces([]));
  }, [draft, source, ctx]);

  const valid = useMemo(() => {
    if (!spec) return false;
    return !!spec.name.trim() && !!spec.target.name.trim() && spec.remotePort > 0 && spec.remotePort < 65536 && (!spec.localPort || spec.localPort < 65536);
  }, [spec]);

  if (!draft || !spec) return null;
  const set = (patch: Partial<ForwardFile>) => setSpec({ ...spec, ...patch });
  const binding = spec.bindEnvironment ?? { variable: 'baseUrl', template: DEFAULT_TEMPLATE };
  const root = project?.root;

  const finalSpec = (): ForwardFile => {
    const s: ForwardFile = { ...spec, name: spec.name.trim(), target: { ...spec.target, name: spec.target.name.trim() } };
    if (!s.localPort) delete s.localPort;
    if (!s.context) delete s.context;
    if (!s.namespace) delete s.namespace;
    if (s.source === 'kubernetes') delete s.source;
    if (bind) {
      const b = { ...binding, variable: (binding.variable || 'baseUrl').trim() };
      if (!b.environment) delete b.environment;
      if (!b.template || b.template === DEFAULT_TEMPLATE) delete b.template;
      s.bindEnvironment = b;
    } else delete s.bindEnvironment;
    return s;
  };

  const run = async (mode: 'start' | 'save' | 'saveStart') => {
    if (!root) return;
    setBusy(true);
    try {
      let id = draft.id;
      const s = finalSpec();
      if (mode !== 'start') {
        id = await infraApi.forward.save(root, draft.id, s);
        useInfraUi.getState().bumpForwards();
        toast.success(`Saved connections/${id}.forward.yaml`);
      }
      if (mode !== 'save') {
        const st = await startForward(root, id, mode === 'start' ? s : undefined);
        if (!st) return;
      }
      close();
    } catch (e) {
      toast.error('Could not save the forward', errorMessage(e));
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
          <Cable size={16} /> {draft.id ? `Edit forward · ${draft.id}` : 'Port-forward'}
        </span>
      }
      footer={
        <>
          <Checkbox checked={saveFile} onChange={setSaveFile} label={draft.id ? 'Save changes to the file' : 'Save to connections/ (shared via git)'} />
          <span className="grow" />
          <button className="btn" onClick={close}>
            Cancel
          </button>
          {saveFile && (
            <button className="btn" disabled={!valid || busy || !root} onClick={() => run('save')}>
              <Save /> Save
            </button>
          )}
          <button className="btn btn-primary" disabled={!valid || busy || !root} onClick={() => run(saveFile ? 'saveStart' : 'start')}>
            <Cable /> {saveFile ? 'Save & start' : 'Start'}
          </button>
        </>
      }
    >
      {!root && <Callout kind="warning">Open a project: forwards bind variables of a project's environments.</Callout>}
      <div className="forward-form">
        <Field label="Name" className="wide">
          <input className="input" value={spec.name} onChange={(e) => set({ name: e.target.value })} autoFocus />
        </Field>
        <Field label="Source">
          <Segmented
            value={source}
            onChange={(v) => set({ source: v, target: { kind: v === 'docker' ? 'container' : 'service', name: '' } })}
            options={[
              { value: 'kubernetes', label: 'Kubernetes' },
              { value: 'docker', label: 'Docker' },
            ]}
          />
        </Field>
        {source === 'kubernetes' ? (
          <>
            <Field label="Context">
              <select className="select" value={ctx} onChange={(e) => set({ context: e.target.value })}>
                {(contexts?.contexts ?? []).map((c) => (
                  <option key={c.name} value={c.name}>
                    {c.name}
                    {c.current ? ' (current)' : ''}
                  </option>
                ))}
                {!contexts && ctx && <option value={ctx}>{ctx}</option>}
              </select>
            </Field>
            <Field label="Namespace" hint="Empty = the context's default">
              <input className="input" list="fwd-namespaces" value={spec.namespace ?? ''} onChange={(e) => set({ namespace: e.target.value })} />
              <datalist id="fwd-namespaces">
                {namespaces.map((n) => (
                  <option key={n} value={n} />
                ))}
              </datalist>
            </Field>
            <Field label="Target">
              <select className="select" value={spec.target.kind} onChange={(e) => set({ target: { ...spec.target, kind: e.target.value as TargetKind } })}>
                {KUBE_KINDS.map((k) => (
                  <option key={k.value} value={k.value}>
                    {k.label}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Target name" hint={spec.target.kind === 'service' ? 'Resolved to a ready pod via its selector; re-resolved when the pod restarts' : undefined}>
              <input className="input mono" value={spec.target.name} onChange={(e) => set({ target: { ...spec.target, name: e.target.value } })} />
            </Field>
          </>
        ) : (
          <Field label="Container" className="wide" hint="The published host port of the container port below is bound (and followed if the container restarts with a new port).">
            <input className="input mono" list="fwd-containers" value={spec.target.name} onChange={(e) => set({ target: { kind: 'container', name: e.target.value } })} />
            <datalist id="fwd-containers">
              {containers.map((c) => (
                <option key={c.id} value={c.name} />
              ))}
            </datalist>
          </Field>
        )}
        <Field label={source === 'docker' ? 'Container port' : 'Remote port'}>
          <input className="input mono" type="number" min={1} max={65535} value={spec.remotePort || ''} onChange={(e) => set({ remotePort: Number(e.target.value) })} />
        </Field>
        {source === 'kubernetes' && (
          <Field label="Local port" hint="Empty = any free port">
            <input className="input mono" type="number" min={0} max={65535} value={spec.localPort ?? ''} placeholder="auto" onChange={(e) => set({ localPort: e.target.value ? Number(e.target.value) : undefined })} />
          </Field>
        )}
      </div>
      <div className="forward-bind mt-4">
        <Checkbox checked={bind} onChange={setBind} label={<span className="strong">Bind the forwarded URL to an environment variable</span>} />
        {bind && (
          <div className="forward-form mt-2">
            <Field label="Environment">
              <select className="select" value={binding.environment ?? ''} onChange={(e) => set({ bindEnvironment: { ...binding, environment: e.target.value || undefined } })}>
                <option value="">Whichever is active</option>
                {(project?.environments ?? []).map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.file.name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Variable">
              <input className="input mono" value={binding.variable} onChange={(e) => set({ bindEnvironment: { ...binding, variable: e.target.value } })} />
            </Field>
            <Field label="Template" className="wide" hint="{port} {host} {remotePort} {name} {namespace} {target}; {{vars}} pass through">
              <input className="input mono" value={binding.template ?? DEFAULT_TEMPLATE} onChange={(e) => set({ bindEnvironment: { ...binding, template: e.target.value } })} />
            </Field>
            <div className="wide xs muted">
              While it runs, <code className="code-inline">{`{{${binding.variable || 'baseUrl'}}}`}</code> resolves to <code className="code-inline">{preview(binding.template ?? DEFAULT_TEMPLATE, spec)}</code> — in memory only; the environment file changes only when you click "Save to environment".
            </div>
          </div>
        )}
      </div>
      {source === 'docker' && (
        <div className="xs subtle mt-4 row-sm">
          <Container size={12} /> Docker ports are already on localhost; nothing is proxied.
        </div>
      )}
    </Modal>
  );
}
