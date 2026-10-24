import { ApiError } from '@/features/mocks/lib/api';
import { proxyStatus } from '@/features/mocks/lib/transport';
import { Callout, CopyButton } from '@/features/mocks/components/ui';
import { startupCommands } from '@/features/mocks/settings/startup';

/** Explains a failed connection to a remote WireMock and suggests concrete fixes. */
export function ConnectionDoctor({ error, url, hasAuth }: { error: ApiError; url: string; hasAuth: boolean }) {
  const viaWebview = proxyStatus() === false;
  const cmds = startupCommands({ port: portOf(url), cors: true });
  return (
    <div className="col">
      <Callout kind="danger">
        <div className="strong">{error.message}</div>
        {error.status && <div className="small muted">HTTP {error.status}</div>}
      </Callout>

      {error.kind === 'credentials' ? (
        <Callout kind="warning">
          <p className="strong">Credentials are not available</p>
          <p>
            The secret for this server is kept in the OS keychain. Either register the <code className="code-inline">wiremock_admin</code> command so the app can read it on the Rust
            side, or enter the credential for this session in <b>Servers</b>.
          </p>
        </Callout>
      ) : error.kind === 'http' && (error.status === 401 || error.status === 403) ? (
        <Callout kind="warning">
          <p className="strong">The admin API requires authentication</p>
          <p>
            {hasAuth
              ? 'The configured credentials were rejected. Check the username, and update the secret in the keychain (Servers → Edit → Credential).'
              : 'WireMock was started with --admin-api-basic-auth (or sits behind an auth proxy). Add authentication to this server in Servers.'}
          </p>
        </Callout>
      ) : error.kind === 'http' && error.status === 404 ? (
        <Callout kind="warning">
          <p className="strong">Admin API not found at this path</p>
          <p>
            The server answered, but not with the WireMock admin API. If WireMock runs under a context path (e.g. behind a gateway at <code className="code-inline">/mocks</code>), set the
            context path in the server settings.
          </p>
        </Callout>
      ) : error.kind === 'timeout' ? (
        <Callout kind="warning">
          <p className="strong">The server did not answer in time</p>
          <p>It may be overloaded, paused in a debugger, or a firewall is silently dropping traffic. VPN or port-forward down?</p>
        </Callout>
      ) : error.kind === 'tls' ? (
        <Callout kind="warning">
          <p className="strong">TLS handshake failed</p>
          <p>
            The certificate is not trusted (self-signed or internal CA). For a test environment you can tick <b>Accept invalid certificates</b> in the server settings, or add the CA to
            the OS trust store.
          </p>
        </Callout>
      ) : (
        <Callout kind="info">
          <p className="strong">Network error — likely causes, most common first</p>
          <ol>
            <li>WireMock is not running, or the host / port is wrong.</li>
            <li>
              The host is only reachable through a VPN or tunnel. Try <code className="code-inline">kubectl port-forward svc/wiremock 8080:8080</code> or{' '}
              <code className="code-inline">ssh -L 8080:host:8080 you@bastion</code> and connect to <code className="code-inline">http://localhost:8080</code>.
            </li>
            {viaWebview && (
              <li>
                Calls go from the webview because the <code className="code-inline">wiremock_admin</code> command is not registered: the app’s content security policy must allow the
                server in <code className="code-inline">connect-src</code>, and the server must answer CORS. Registering the command removes both requirements.
              </li>
            )}
            <li>A proxy or gateway in front of WireMock rejects the request.</li>
          </ol>
        </Callout>
      )}

      <div className="panel">
        <div className="panel-header">
          <h3 className="grow">Start a WireMock server</h3>
        </div>
        <div className="panel-body col">
          {cmds.slice(0, 2).map((c) => (
            <div key={c.label} className="col-sm">
              <div className="row between">
                <span className="label">{c.label}</span>
                <CopyButton text={c.cmd} />
              </div>
              <pre className="code">{c.cmd}</pre>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function portOf(url: string): number {
  try {
    const u = new URL(url.includes('://') ? url : `http://${url}`);
    return Number(u.port) || (u.protocol === 'https:' ? 443 : 8080);
  } catch {
    return 8080;
  }
}
