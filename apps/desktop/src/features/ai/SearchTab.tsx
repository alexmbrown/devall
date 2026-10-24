import { useCallback, useEffect, useMemo, useState } from 'react';
import { Box, Database, FileText, Layers, Play, RefreshCw, Search, Send, Server, Square, Trash2 } from 'lucide-react';
import { Callout, Empty, Field, formatBytes, RelTime, SearchInput, Spinner } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { aiApi, MANAGED_EMBED_ID, type CatalogEntry, type IndexProgress, type IndexView, type SearchHit, type SearchResult } from '@/lib/api/ai';
import { errorMessage } from '@/lib/ipc';
import { useProjectLinkOpener } from '@/features/notes/NotesPage';
import { useNav } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { isLocalProvider, newCallId, useAi } from './store';

const KIND_LABEL: Record<string, string> = { note: 'Notes', request: 'Requests', mock: 'Mocks', environment: 'Environments' };

export function HitIcon({ kind }: { kind: string }) {
  if (kind === 'request') return <Send size={13} />;
  if (kind === 'mock') return <Server size={13} />;
  if (kind === 'environment') return <Layers size={13} />;
  return <FileText size={13} />;
}

/** One search result: kind icon, title (and section), snippet, similarity. */
export function HitRow({ hit, onOpen }: { hit: SearchHit; onOpen(): void }) {
  return (
    <button className="ai-hit" onClick={onOpen} title={hit.source}>
      <HitIcon kind={hit.kind} />
      <span className="grow" style={{ minWidth: 0 }}>
        <span className="ai-hit-title truncate">
          {hit.title}
          {hit.anchor && hit.kind === 'note' && hit.anchor !== hit.title && <span className="subtle"> › {hit.anchor}</span>}
        </span>
        <span className="ai-hit-snippet">{hit.snippet}</span>
      </span>
      {hit.similarity !== undefined && <span className="badge" title="Cosine similarity with the question">{Math.round(hit.similarity * 100)}%</span>}
    </button>
  );
}

/** Local semantic search: embedding model, the project's index (in its data folder on this computer) and a search box. */
export function SearchTab() {
  const project = useWorkspace((s) => s.project);
  const { view: providers, load: loadProviders } = useAi();
  const go = useNav((s) => s.go);
  const openLink = useProjectLinkOpener();
  const [index, setIndex] = useState<IndexView | null>(null);
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [provider, setProvider] = useState('');
  const [model, setModel] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [build, setBuild] = useState<{ callId: string; progress?: IndexProgress } | null>(null);
  const [q, setQ] = useState('');
  const [result, setResult] = useState<SearchResult | null>(null);
  const [searching, setSearching] = useState(false);

  const root = project?.root;
  const refresh = useCallback(async () => {
    if (!root) return;
    try {
      const v = await aiApi.indexStatus(root);
      setIndex(v);
      setProvider(v.settings?.provider ?? '');
      setModel(v.settings?.model ?? '');
    } catch (e) {
      toast.error('Could not read the search index', errorMessage(e));
    }
  }, [root]);

  useEffect(() => {
    void refresh();
    aiApi
      .catalog()
      .then((c) => setCatalog(c.models.filter((m) => m.role === 'embedding')))
      .catch(() => {});
  }, [refresh]);

  // Search as you type.
  useEffect(() => {
    if (!root || !q.trim()) {
      setResult(null);
      return;
    }
    const t = setTimeout(() => {
      setSearching(true);
      aiApi
        .search(root, q, 25)
        .then(setResult)
        .catch((e) => setResult({ mode: 'keyword', hits: [], note: errorMessage(e) }))
        .finally(() => setSearching(false));
    }, 250);
    return () => clearTimeout(t);
  }, [q, root, index?.status.builtAt]);

  const embedProviders = useMemo(() => (providers?.config.providers ?? []).filter((p) => p.kind === 'ollama' || p.kind === 'openAiCompatible'), [providers]);
  const chosen = embedProviders.find((p) => p.id === provider);
  const downloaded = catalog.filter((m) => m.status.downloaded);
  const modelHints = provider === MANAGED_EMBED_ID ? catalog.map((m) => m.id) : chosen?.kind === 'ollama' ? catalog.map((m) => m.ollamaTag).filter((t): t is string => !!t) : [];

  if (!project)
    return (
      <Empty icon={<Search />} title="Open a project to search it">
        Notes, requests, mocks and environment names are embedded with a local model and searched by meaning.
      </Empty>
    );

  const saveSettings = async (p = provider, m = model) => {
    try {
      await aiApi.embeddingSettings(p, m);
      await refresh();
      toast.success(p ? 'Embedding model saved' : 'Embedding model cleared');
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  const startManaged = async (id: string) => {
    setBusy('managed');
    try {
      await aiApi.embedSidecarStart(id);
      if (provider !== MANAGED_EMBED_ID || model !== id) await aiApi.embeddingSettings(MANAGED_EMBED_ID, id);
      await refresh();
      void loadProviders();
    } catch (e) {
      toast.error('Could not start the embedding model', errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const stopManaged = async () => {
    setBusy('managed');
    try {
      await aiApi.embedSidecarStop();
      await refresh();
    } finally {
      setBusy(null);
    }
  };

  const runBuild = async () => {
    if (!root) return;
    const callId = newCallId();
    setBuild({ callId });
    try {
      const out = await aiApi.indexBuild(root, (progress) => setBuild({ callId, progress }), callId);
      if (out.cancelled) toast.info('Indexing stopped', `${out.status.chunks} chunks kept — run it again to continue.`);
      else toast.success('Search index ready', `${out.embedded} embedded, ${out.reused} unchanged`);
    } catch (e) {
      toast.error('Indexing failed', errorMessage(e));
    } finally {
      setBuild(null);
      await refresh();
    }
  };

  const clear = async () => {
    if (!root || !(await confirmDialog({ title: 'Delete the search index?', message: 'It is a cache on this computer (the project data folder) — building it again re-embeds everything.', confirmText: 'Delete', danger: true }))) return;
    try {
      await aiApi.indexClear(root);
      await refresh();
    } catch (e) {
      toast.error('Could not delete the index', errorMessage(e));
    }
  };

  const st = index?.status;
  const p = build?.progress;
  const pct = p?.type === 'embedded' ? (p.toEmbed ? (p.done / p.toEmbed) * 100 : 100) : p?.type === 'saved' ? 100 : 0;
  const stale = (st?.stale ?? 0) + (st?.removed ?? 0);
  const managed = index?.managed;

  return (
    <div className="col" style={{ gap: 'var(--space-7)', maxWidth: 960 }}>
      <section className="ai-section">
        <h2>
          <Search /> Search the project
        </h2>
        <SearchInput value={q} onChange={setQ} placeholder="Ask in plain words: “where do refunds happen?”, “staging auth token”…" autoFocus />
        {result?.note && <Callout kind={result.mode === 'keyword' ? 'warning' : 'info'}>{result.note}</Callout>}
        {searching && !result && <Spinner />}
        {result && result.hits.length === 0 && !searching && <div className="subtle small">Nothing found.</div>}
        {result && result.hits.length > 0 && (
          <div className="card ai-hits">
            {result.hits.map((h, i) => (
              <HitRow key={`${h.source}:${h.anchor ?? ''}:${i}`} hit={h} onOpen={() => openLink(h.source)} />
            ))}
          </div>
        )}
      </section>

      <section className="ai-section">
        <h2>
          <Database /> Index
        </h2>
        <div className="card ai-row">
          <div className="grow">
            <div className="ai-row-title">
              {st?.exists ? `${st.chunks} chunks` : 'Not built yet'}
              {st?.exists && Object.entries(st.byKind).map(([k, n]) => <span key={k} className="badge">{`${KIND_LABEL[k] ?? k} ${n}`}</span>)}
              {st?.partial && <span className="badge warning">incomplete</span>}
              {st?.exists && index && !index.embedderMatches && index.embedder && <span className="badge warning" title={`Built with ${st.embedderLabel ?? st.embedder}`}>other model</span>}
            </div>
            <div className="ai-row-sub">
              {st?.exists ? (
                <>
                  {st.embedderLabel ?? st.embedder} · {st.dims}-d · {formatBytes(st.sizeBytes)} · built <RelTime ts={st.builtAt} />
                  {stale > 0 ? ` · ${stale} change${stale === 1 ? '' : 's'} since` : ' · up to date'}
                </>
              ) : (
                `${st?.stale ?? 0} chunks to embed — stored on this computer, never in project files`
              )}
            </div>
            {build && (
              <div className="row-sm mt-2">
                <div className="ai-progress">
                  <span style={{ width: `${pct}%` }} />
                </div>
                <span className="xs subtle">
                  {p?.type === 'scanned' ? `${p.toEmbed} to embed, ${p.reused} unchanged` : p?.type === 'embedded' ? `${p.done} / ${p.toEmbed}` : p?.type === 'saved' ? 'saving…' : 'scanning…'}
                </span>
              </div>
            )}
          </div>
          {build ? (
            <button className="btn btn-sm" onClick={() => void aiApi.cancel(build.callId)}>
              <Square /> Stop
            </button>
          ) : (
            <>
              <button className="btn btn-sm btn-primary" onClick={() => void runBuild()} disabled={!index?.embedder} title={index?.embedderError ?? 'Embed new and changed chunks'}>
                <RefreshCw /> {st?.exists ? 'Update' : 'Build'}
              </button>
              {st?.exists && (
                <button className="icon-btn" onClick={() => void clear()} title="Delete the index" aria-label="Delete the index">
                  <Trash2 />
                </button>
              )}
            </>
          )}
        </div>
        {index?.embedderError && <Callout kind="warning">{index.embedderError}</Callout>}
        {index?.embedder && !index.embedderLocal && <Callout kind="warning">The embedding model is a cloud endpoint: note, request and mock text is sent to it (you allowed project context for cloud providers).</Callout>}
      </section>

      <section className="ai-section">
        <h2>
          <Box /> Embedding model
        </h2>
        <div className="grid-2">
          <Field label="Provider" hint="Local by default: Ollama, LM Studio, llama.cpp (--embedding) or the managed model below.">
            <select className="select" value={provider} onChange={(e) => setProvider(e.target.value)}>
              <option value="">Automatic (the managed embedding model when running)</option>
              <option value={MANAGED_EMBED_ID}>Managed llama.cpp embedding model</option>
              {embedProviders.map((pr) => (
                <option key={pr.id} value={pr.id}>
                  {pr.name} {isLocalProvider(pr) ? '· local' : '· cloud'}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Model" hint={provider === MANAGED_EMBED_ID ? 'A catalog embedding model id' : chosen?.kind === 'ollama' ? 'Pull it first: ollama pull embeddinggemma-2:270m' : 'The server’s embedding model name'}>
            <input className="input mono" list="ai-embed-models" value={model} onChange={(e) => setModel(e.target.value)} placeholder={provider === MANAGED_EMBED_ID ? 'embeddinggemma-2' : 'embeddinggemma-2:270m'} disabled={!provider} />
            <datalist id="ai-embed-models">
              {modelHints.map((m) => (
                <option key={m} value={m} />
              ))}
            </datalist>
          </Field>
        </div>
        <div className="row-sm">
          <button className="btn btn-sm" onClick={() => void saveSettings()} disabled={provider === (index?.settings?.provider ?? '') && model === (index?.settings?.model ?? '')}>
            Save
          </button>
          {chosen && !isLocalProvider(chosen) && <span className="small text-warning">Cloud embeddings need “Allow project context for cloud providers” (AI → Models).</span>}
        </div>
        <div className="card">
          {catalog.map((m) => {
            const running = managed?.running && managed.model === m.id;
            return (
              <div key={m.id} className="ai-row">
                <div className="grow">
                  <div className="ai-row-title">
                    {m.name}
                    {running && <span className="badge success">running</span>}
                    {!m.status.downloaded && <span className="badge">not downloaded</span>}
                  </div>
                  <div className="ai-row-sub">
                    {formatBytes(m.sizeBytes)} · {m.notes}
                  </div>
                </div>
                {running ? (
                  <button className="btn btn-sm" onClick={() => void stopManaged()} disabled={busy === 'managed'}>
                    <Square /> Stop
                  </button>
                ) : m.status.downloaded ? (
                  <button className="btn btn-sm" onClick={() => void startManaged(m.id)} disabled={busy === 'managed'}>
                    {busy === 'managed' ? <Spinner /> : <Play />} Start
                  </button>
                ) : (
                  <button className="btn btn-sm btn-ghost" onClick={() => go('ai', 'models')}>
                    Download in Models
                  </button>
                )}
              </div>
            );
          })}
          {catalog.length === 0 && <div className="ai-row subtle small">The model catalog is unavailable.</div>}
        </div>
        {downloaded.length === 0 && catalog.length > 0 && <div className="subtle small">EmbeddingGemma 2 is ~300 MB. Ollama users can instead run <code className="code-inline">ollama pull embeddinggemma-2:270m</code>.</div>}
      </section>
    </div>
  );
}
