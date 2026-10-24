import { useCallback, useEffect, useState } from 'react';
import { Bot, Globe, Lock, Play, Square, Terminal } from 'lucide-react';
import { Callout, CopyButton, Empty, Segmented, Spinner, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { aiApi, type HttpInfo, type McpStatus, type McpWriteMode } from '@/lib/api/ai';
import { writeFlags } from './approvals';
import { errorMessage } from '@/lib/ipc';
import { useWorkspace } from '@/store/workspace';

const TOOLS: [string, string][] = [
  ['get_context', 'Notes, your to-dos, the request outline and environments, ranked for a query'],
  ['list_requests / read_request', 'Saved requests (YAML) with effective method and URL'],
  ['send_request', 'Send a saved request — GET/HEAD/OPTIONS only unless writes are allowed or approved; protected environments always need your approval'],
  ['list_environments', 'Variables and secret names (never values)'],
  ['list_mocks', 'WireMock stubs in mocks/mappings'],
  ['search_notes / read_note', 'Shared Markdown notes (also exposed as resources)'],
  ['write_note / append_to_note / edit_note', 'Create and edit shared Markdown notes (unless notes & to-dos are read-only)'],
  ['list_todos', "Your personal to-do list for this project (other lists stay private)"],
  ['add_todo / update_todo / complete_todo / comment_todo', 'Keep this project’s to-dos up to date — quick-add syntax works (unless read-only)'],
  ['run_utility', 'Encoders, JWT decode, hashes, ids, timestamps, JSON/YAML, regex, JSONPath…'],
];

const LOCAL_WRITES_KEY = 'devall.mcp.localWrites';

function readLocalWrites(): boolean {
  try {
    return localStorage.getItem(LOCAL_WRITES_KEY) !== 'false';
  } catch {
    return true;
  }
}

function Snippet({ title, text, hint }: { title: string; text: string; hint?: string }) {
  return (
    <div className="col-sm">
      <div className="strong small">{title}</div>
      {hint && <div className="xs subtle">{hint}</div>}
      <div className="ai-snippet">
        <pre className="ai-pre">{text}</pre>
        <CopyButton text={text} />
      </div>
    </div>
  );
}

export function AgentsTab() {
  const project = useWorkspace((s) => s.project);
  const [mode, setMode] = useState<McpWriteMode>('readOnly');
  const { allowWrites, confirmWrites } = writeFlags(mode);
  const [localWrites, setLocalWritesState] = useState(readLocalWrites);
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [http, setHttp] = useState<HttpInfo | null>(null);
  const [busy, setBusy] = useState(false);

  const setLocalWrites = (v: boolean) => {
    setLocalWritesState(v);
    try {
      localStorage.setItem(LOCAL_WRITES_KEY, String(v));
    } catch {
      /* storage unavailable */
    }
    if (http) toast.info('Restart the HTTP endpoint', 'The change applies after Stop → Start.');
  };

  const refresh = useCallback(() => {
    if (!project) return;
    aiApi
      .mcpStatus(project.root, allowWrites, localWrites)
      .then((s) => {
        setStatus(s);
        setHttp(s.http);
      })
      .catch((e) => toast.error('MCP status failed', errorMessage(e)));
  }, [project, allowWrites, localWrites]);

  useEffect(refresh, [refresh]);

  if (!project)
    return (
      <Empty icon={<Bot />} title="Open a project to connect agents">
        Coding agents (Claude Code, Cursor, VS Code…) can read this project's requests, notes and to-dos through MCP.
      </Empty>
    );

  const toggleHttp = async () => {
    setBusy(true);
    try {
      if (http) {
        await aiApi.mcpHttpStop();
        setHttp(null);
      } else setHttp(await aiApi.mcpHttpStart(project.root, allowWrites, confirmWrites, localWrites));
    } catch (e) {
      toast.error('MCP HTTP server', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const httpJson = http
    ? JSON.stringify({ mcpServers: { devall: { type: 'http', url: http.url, headers: { Authorization: `Bearer ${http.token}` } } } }, null, 2)
    : '';

  return (
    <div className="col-lg" style={{ maxWidth: 900, margin: '0 auto', width: '100%' }}>
      <Callout>
        The <strong>devall MCP server</strong> lets coding agents use this project: list and send saved requests, read and write notes and your to-dos, read mocks and
        environments, and run utilities. Every output has secret values and credential-like strings masked. It runs locally — over stdio, started by the agent.
      </Callout>

      <section className="ai-section">
        <h2>
          <Lock /> Permissions
        </h2>
        <div className="row-sm wrap">
          <span className="small">Writes (POST/PUT/PATCH/DELETE)</span>
          <Segmented<McpWriteMode>
            value={mode}
            onChange={(m) => {
              setMode(m);
              if (http) toast.info('Restart the HTTP endpoint', 'The new write mode applies after Stop → Start.');
            }}
            options={[
              { value: 'readOnly', label: 'Read-only', title: 'send_request only sends GET, HEAD and OPTIONS' },
              { value: 'confirm', label: 'Ask each time', title: 'Each write waits for your approval in this app (HTTP endpoint)' },
              { value: 'allow', label: 'Allowed', title: 'Writes are sent without asking (adds --allow-writes)' },
            ]}
          />
        </div>
        <div className="xs subtle">
          {mode === 'readOnly' && (
            <>
              <span className="mono xs">send_request</span> only sends GET, HEAD and OPTIONS.{' '}
            </>
          )}
          {mode === 'confirm' && (
            <>
              Each write shows an approval prompt here (method, URL, environment, body — secrets masked): <strong>Allow once</strong>, <strong>Allow for this session</strong> or{' '}
              <strong>Deny</strong>; no answer within 2 minutes denies it. This works with the app's HTTP endpoint below — agents that start{' '}
              <span className="mono xs">devall mcp</span> themselves (stdio) have no app to ask, so they stay read-only.{' '}
            </>
          )}
          {mode === 'allow' && <>Agents may send any method without asking (stdio snippets get <span className="mono xs">--allow-writes</span>). </>}
          Requests in environments marked <strong>protected</strong> always need your approval through the HTTP endpoint and are never sent over stdio. Secrets are resolved from your
          keychain without ever being shown to the agent.
        </div>
        <Switch
          checked={localWrites}
          onChange={setLocalWrites}
          label={<span className="small">Agents can write notes &amp; to-dos</span>}
          title="write_note, append_to_note, edit_note, add_todo, update_todo, complete_todo, comment_todo"
        />
        <div className="xs subtle">
          {localWrites ? (
            <>
              Agents can create and edit shared Markdown notes (<span className="mono xs">notes/</span>, committed with your repo) and add, update, complete and comment on this
              project’s to-dos (stored on this machine; your other lists stay private). They can’t delete either, and notes with masked secrets can only be edited in parts, so a
              value the agent never saw is never overwritten.
            </>
          ) : (
            <>
              Notes and to-dos are read-only for agents (stdio snippets get <span className="mono xs">--read-only-notes</span>).
            </>
          )}
        </div>
      </section>

      <section className="ai-section">
        <h2>
          <Terminal /> Connect
        </h2>
        {status && !status.cli && (
          <Callout kind="warning">
            The <span className="mono xs">devall</span> command-line tool wasn't found on your PATH. Install it (it ships with the app) so agents can start{' '}
            <span className="mono xs">devall mcp</span>.
          </Callout>
        )}
        {status ? (
          <>
            <Snippet title="Claude Code" text={status.snippets.claudeCode} hint="Run in a terminal; the server starts when Claude Code needs it." />
            <Snippet title="Cursor / Claude Desktop / Windsurf" text={status.snippets.mcpServersJson} hint="Add to .cursor/mcp.json (or the client's MCP settings)." />
            <Snippet title="VS Code" text={status.snippets.vscodeJson} hint="Add to .vscode/mcp.json." />
          </>
        ) : (
          <Spinner />
        )}
      </section>

      <section className="ai-section">
        <h2>
          <Globe /> HTTP endpoint (optional)
          <span className="spacer" />
          <button className="btn btn-sm" onClick={toggleHttp} disabled={busy}>
            {busy ? <Spinner /> : http ? <Square /> : <Play />} {http ? 'Stop' : 'Start'}
          </button>
        </h2>
        <div className="xs subtle">
          For clients that can't launch a command. Listens on 127.0.0.1 only, on a random port, requires a random bearer token, and rejects browser origins (DNS-rebinding
          protection). It stops when the app closes.
        </div>
        {http && <Snippet title={`Streamable HTTP · ${http.url}`} text={httpJson} hint="The token changes every time the endpoint starts." />}
      </section>

      <section className="ai-section">
        <h2>
          <Bot /> Tools
        </h2>
        <div className="panel">
          {TOOLS.map(([name, desc]) => (
            <div key={name} className="ai-row">
              <span className="mono xs" style={{ minWidth: 220 }}>
                {name}
              </span>
              <span className="small muted grow">{desc}</span>
            </div>
          ))}
        </div>
        <div className="xs subtle">
          Speaks MCP 2026-07-28 (stateless, <span className="mono xs">server/discover</span>) and the earlier <span className="mono xs">initialize</span>-based revisions.
        </div>
      </section>
    </div>
  );
}
