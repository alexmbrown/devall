import { useCallback, useEffect, useState } from 'react';
import { BadgeCheck, CloudUpload, Globe, RefreshCw, ShieldAlert, ShieldCheck, Trash2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, CopyButton, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { cloudTargetFor as targetFor, loadApi, type CloudLoadInfo, type CloudPrepare, type CloudRun } from '@/lib/api/load';
import { useWorkspace } from '@/store/workspace';

const REGIONS_KEY = 'devall.load.cloudRegions';

function savedRegions(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(REGIONS_KEY) ?? '[]') as unknown;
    if (Array.isArray(v)) return v.filter((x): x is string => typeof x === 'string');
  } catch {
    /* per-device convenience only */
  }
  return ['weur', 'enam'];
}

interface Props {
  open: boolean;
  onClose(): void;
  root: string;
  path: string;
  onRun(c: CloudRun): void;
}

/**
 * "Run from cloud regions…": the relay's load generators (Supporter, or any self-hosted relay).
 * Every target host must prove it's yours first (a token file or DNS TXT record), the region
 * picker is capped by the plan, and secrets need explicit consent like distributed agents.
 */
export function CloudRegionsDialog({ open, onClose, root, path, onRun }: Props) {
  const [info, setInfo] = useState<CloudLoadInfo | null>(null);
  const [prep, setPrep] = useState<CloudPrepare | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [regions, setRegions] = useState<string[]>(savedRegions);
  const [allowSecrets, setAllowSecrets] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const env = useWorkspace.getState().activeEnv()?.id;
      const [i, p] = await Promise.all([loadApi.cloudInfo(), loadApi.cloudPrepare(root, path, env)]);
      setInfo(i);
      setPrep(p);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [root, path]);

  useEffect(() => {
    if (!open) return;
    setInfo(null);
    setPrep(null);
    setAllowSecrets(false);
    void refresh();
  }, [open, refresh]);

  const act = async (key: string, fn: () => Promise<unknown>) => {
    setBusy(key);
    try {
      await fn();
      await refresh();
    } catch (e) {
      toast.error('Verification', errorMessage(e));
      await refresh();
    } finally {
      setBusy(null);
    }
  };

  const max = info?.limits.loadRegions ?? 0;
  const targets = info?.targets ?? [];
  const origins = prep?.origins ?? [];
  const unverified = origins.filter((o) => targetFor(targets, o)?.status !== 'verified');
  const secrets = prep?.secretRequests ?? [];
  const needsConsent = secrets.length > 0 && !allowSecrets;
  const chosen = regions.filter((r) => info?.regions.some((x) => x.id === r)).slice(0, Math.max(1, max));
  const overDuration = !!prep && !!info && prep.plannedDurationMs > info.limits.loadMaxDurationSeconds * 1000;
  const blocked = !info || !prep || !info.enabled || max === 0 || !!prep.error || unverified.length > 0 || chosen.length === 0 || needsConsent || overDuration;

  const toggle = (id: string, on: boolean) =>
    setRegions((list) => {
      const next = on ? [...list.filter((r) => r !== id), id] : list.filter((r) => r !== id);
      // Keep within the plan: drop the oldest pick.
      return max > 0 && next.length > max ? next.slice(next.length - max) : next;
    });

  const run = () => {
    try {
      localStorage.setItem(REGIONS_KEY, JSON.stringify(chosen));
    } catch {
      /* per-device convenience only */
    }
    onRun({ regions: chosen, allowSecrets });
    onClose();
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      size="lg"
      title="Run from cloud regions"
      footer={
        <>
          <button className="btn" onClick={() => void refresh()} disabled={!!busy}>
            <RefreshCw /> Refresh
          </button>
          <span className="grow" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={run} disabled={blocked} title={unverified.length ? 'Verify every target host first' : needsConsent ? 'Allow sending resolved secrets first' : undefined}>
            <CloudUpload /> Run from {chosen.length || 'no'} region{chosen.length === 1 ? '' : 's'}
          </button>
        </>
      }
    >
      <div className="col">
        <div className="subtle small">
          The relay's edge sends this test's open-model load from Durable Objects placed in the regions you pick, merges the results and saves the run here like a local one. Placement is
          best-effort (<code className="code-inline">locationHint</code>); each region's data center is listed in the result when the relay can tell. Load only goes to hosts you proved you
          control, and every request carries <code className="code-inline">x-devall-load: &lt;run id&gt;</code>.
        </div>
        {error && <Callout kind="danger">{error}</Callout>}
        {!info || !prep ? (
          !error && <Spinner />
        ) : (
          <>
            {!info.enabled && <Callout kind="warning">Cloud load is switched off on this relay right now (the operator's kill switch). Try again later.</Callout>}
            {max === 0 ? (
              <Callout kind="info">
                <b>Cloud load regions</b> are part of the Supporter plan (~$4/month) — or self-host the relay, where they're free. Locally, <code className="code-inline">Run</code> and{' '}
                <code className="code-inline">Distribute…</code> stay free.
              </Callout>
            ) : (
              <div className="xs subtle">
                Your plan: up to {max} region{max === 1 ? '' : 's'} · {info.limits.loadMaxDurationSeconds} s · {info.limits.loadMaxRps === null ? 'no rate cap' : `${info.limits.loadMaxRps} requests/s in total`} ·{' '}
                {info.limits.loadRunsPerDay === null ? 'unlimited runs' : `${info.runsToday} of ${info.limits.loadRunsPerDay} runs today`}
                {info.limits.loadPrivateTargets ? ' · private / loopback targets allowed (self-hosted)' : ''}
              </div>
            )}
            {prep.error && <Callout kind="warning">{prep.error}</Callout>}
            {overDuration && (
              <Callout kind="warning">
                This test runs {Math.ceil(prep.plannedDurationMs / 1000)} s; cloud runs on your plan last at most {info.limits.loadMaxDurationSeconds} s.
              </Callout>
            )}
            {max > 0 && (
              <Field label="Target hosts" hint="Serve the token at the file URL (HTTP 200, the file holds just the token, no redirect) or add the DNS TXT record; verifications last 30 days.">
                <div className="col-sm">
                  {origins.map((o) => {
                    const t = targetFor(targets, o);
                    return (
                      <div key={o} className="card col-sm" style={{ padding: 8 }}>
                        <div className="row-sm">
                          <Globe size={14} />
                          <span className="mono strong grow truncate">{o}</span>
                          {t?.status === 'verified' ? (
                            <span className="row-sm text-success xs" title={`by ${t.method} until ${t.expiresAt}`}>
                              <BadgeCheck size={14} /> verified{t.expiresAt ? ` until ${new Date(t.expiresAt).toLocaleDateString()}` : ''}
                            </span>
                          ) : t ? (
                            <span className="text-warning xs">{t.status === 'expired' ? 'verification expired' : 'not verified yet'}</span>
                          ) : null}
                          {t ? (
                            <>
                              <button className="btn btn-sm" disabled={!!busy} onClick={() => void act(`v:${o}`, () => loadApi.cloudVerify(o))}>
                                {busy === `v:${o}` ? <Spinner /> : <ShieldCheck />} Check now
                              </button>
                              <button className="icon-btn sm" title="Forget this target" aria-label="Forget this target" disabled={!!busy} onClick={() => void act(`r:${o}`, () => loadApi.cloudRemove(o))}>
                                <Trash2 />
                              </button>
                            </>
                          ) : (
                            <button className="btn btn-sm btn-primary" disabled={!!busy} onClick={() => void act(`t:${o}`, () => loadApi.cloudTarget(o))}>
                              {busy === `t:${o}` ? <Spinner /> : <ShieldCheck />} Get verification token
                            </button>
                          )}
                        </div>
                        {t && t.status !== 'verified' && (
                          <div className="col-sm xs">
                            <div className="row-sm">
                              <span className="subtle nowrap">Token</span>
                              <code className="code-inline grow truncate">{t.token}</code>
                              <CopyButton text={t.token} title="Copy token" />
                            </div>
                            <div className="row-sm">
                              <span className="subtle nowrap">File</span>
                              <code className="code-inline grow truncate">{t.fileUrl}</code>
                              <CopyButton text={t.fileUrl} title="Copy URL" />
                            </div>
                            <div className="row-sm">
                              <span className="subtle nowrap">or DNS</span>
                              <code className="code-inline grow truncate">
                                {t.dnsName} TXT "{t.token}"
                              </code>
                              <CopyButton text={`${t.dnsName} TXT "${t.token}"`} title="Copy record" />
                            </div>
                            {t.lastError && <span className="text-danger">{t.lastError}</span>}
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {!origins.length && <span className="xs subtle">This test has no requests.</span>}
                </div>
              </Field>
            )}
            {max > 0 && (
              <Field label={`Regions (up to ${max})`} hint="Durable Object location hints: the relay asks for an object in each region; Cloudflare decides the exact data center.">
                <div className="grid-3">
                  {info.regions.map((r) => (
                    <Checkbox key={r.id} checked={chosen.includes(r.id)} onChange={(v) => toggle(r.id, v)} label={`${r.name} (${r.id})`} />
                  ))}
                </div>
              </Field>
            )}
            {secrets.length > 0 && max > 0 && (
              <Callout kind="warning">
                <div className="col-sm">
                  <div className="row-sm strong">
                    <ShieldAlert size={14} /> {secrets.length === 1 ? 'A request uses' : `${secrets.length} requests use`} secrets: {secrets.join(', ')}
                  </div>
                  <div className="small">The relay's edge needs the resolved values to send them (they're used for the run only, never stored). Without your consent the run is refused.</div>
                  <Checkbox checked={allowSecrets} onChange={setAllowSecrets} label="Send resolved secrets to the relay (I trust the relay I use)" />
                </div>
              </Callout>
            )}
            {prep.warnings.length > 0 && (
              <ul className="xs subtle" style={{ margin: 0, paddingLeft: 18 }}>
                {prep.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
