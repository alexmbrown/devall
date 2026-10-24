import { useState, type ReactNode } from 'react';
import { PlugZap, RefreshCw, Settings2, Server } from 'lucide-react';
import { useHealth, useWm } from '@/features/mocks/lib/queries';
import { ApiError } from '@/features/mocks/lib/api';
import { useNav } from '@/features/mocks/store/nav';
import { useMockTarget } from '@/features/mocks/store/target';
import { Spinner, Callout } from '@/features/mocks/components/ui';
import { ConnectionDoctor } from './ConnectionDoctor';
import { StartControls } from './ServerBar';

/** Shows the page only once the target answers: a start screen for the built-in engine, diagnostics for a remote one. */
export function ConnectionGate({ children }: { children: ReactNode }) {
  const { server } = useWm();
  const health = useHealth();
  const go = useNav((s) => s.go);
  const status = useMockTarget((s) => s.status);
  const statusLoaded = useMockTarget((s) => s.statusLoaded);
  const [bypass, setBypass] = useState<string | null>(null);

  if (server.kind === 'builtin') {
    if (!statusLoaded)
      return (
        <div className="empty">
          <Spinner />
        </div>
      );
    if (!status?.running)
      return (
        <div className="page-body">
          <div className="col-lg" style={{ maxWidth: 760, margin: '24px auto' }}>
            <div className="row items-start">
              <Server size={26} className="subtle" />
              <div className="grow">
                <h1>Start the mock server</h1>
                <p className="muted small">
                  A WireMock-compatible server for this project. Stubs are the files in <code className="code-inline">mocks/mappings/</code> (WireMock JSON, commit them with your
                  code) and response bodies live in <code className="code-inline">mocks/__files/</code>. It runs on this machine, free, nothing leaves it.
                </p>
              </div>
            </div>
            <div className="panel">
              <div className="panel-body col">
                <StartControls big />
                <div className="subtle xs">Leave the port empty to pick a free one. Point your app or tests at the URL shown once it runs.</div>
              </div>
            </div>
            {(status?.loadErrors.length ?? 0) > 0 && (
              <Callout kind="warning">
                {status!.loadErrors.length} mapping file(s) could not be loaded last time: {status!.loadErrors.map((e) => e.file).join(', ')}
              </Callout>
            )}
            <div className="row-sm subtle xs">
              Already run WireMock elsewhere (a team server, staging)?
              <button className="btn btn-sm btn-ghost" onClick={() => go('servers', 'new')}>
                Connect a remote WireMock
              </button>
            </div>
          </div>
        </div>
      );
    return <>{children}</>;
  }

  if (health.isPending && !health.data)
    return (
      <div className="empty">
        <Spinner />
        <p>Connecting to {server.url}…</p>
      </div>
    );

  if (health.isError && bypass !== server.id) {
    const err = health.error instanceof ApiError ? health.error : new ApiError(String(health.error?.message ?? health.error), 'network');
    return (
      <div className="page-body">
        <div className="col-lg" style={{ maxWidth: 820, margin: '0 auto' }}>
          <div className="row">
            <PlugZap size={22} className="text-danger" />
            <div className="grow">
              <h1>Can't reach {server.name}</h1>
              <div className="muted small mono">{server.url}</div>
            </div>
            <button className="btn" onClick={() => go('servers', server.id)}>
              <Settings2 /> Edit connection
            </button>
            <button className="btn btn-primary" onClick={() => health.refetch()} disabled={health.isFetching}>
              {health.isFetching ? <Spinner /> : <RefreshCw />} Retry
            </button>
          </div>
          <ConnectionDoctor error={err} url={server.url} hasAuth={!!server.auth} />
          <div>
            <button className="btn btn-ghost btn-sm" onClick={() => setBypass(server.id)}>
              Continue anyway
            </button>
          </div>
        </div>
      </div>
    );
  }
  return <>{children}</>;
}
