import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Modal } from '@/components/ui/Modal';
import { Callout, CopyButton, Spinner, Switch } from '@/components/ui/misc';
import { CodeEditor, type CodeLanguage } from '@/components/editor/CodeEditor';
import { errorMessage } from '@/lib/ipc';
import type { HttpRequestFile } from '@/lib/types';
import { importApi, type CodegenLanguage, type LanguageInfo } from '@/lib/api/import';
import { useWorkspace } from '@/store/workspace';
import { isPluginOp, usePluginCatalog } from '@/lib/api/plugins';
import './import.css';

export interface GenerateCodeModalProps {
  open: boolean;
  onClose(): void;
  /** Request location (for folder inheritance); '' for unsaved drafts. */
  path: string;
  /** The editor's current state (may be unsaved). */
  request: HttpRequestFile;
  /** Environment id; defaults to the active environment. */
  env?: string;
}

const LANG_KEY = 'devall.codegenLanguage';
const EDITOR_LANGS: CodeLanguage[] = ['shell', 'powershell', 'javascript', 'python', 'go', 'rust', 'java', 'kotlin', 'csharp', 'php', 'ruby', 'swift'];
let languagesCache: LanguageInfo[] | null = null;

function savedLanguage(): CodegenLanguage {
  try {
    return (localStorage.getItem(LANG_KEY) as CodegenLanguage | null) ?? 'curl';
  } catch {
    return 'curl';
  }
}

/** Code snippets for a request in 15 languages, resolved like a send; secrets masked by default. */
export function GenerateCodeModal({ open, onClose, path, request, env }: GenerateCodeModalProps) {
  const project = useWorkspace((s) => s.project);
  const activeEnv = useWorkspace((s) => s.activeEnv());
  const envId = env ?? activeEnv?.id;
  const [builtin, setLanguages] = useState<LanguageInfo[]>(languagesCache ?? []);
  // Codegen targets from plugins (`<plugin>:<target>`) in their own group.
  const pluginTargets = usePluginCatalog((s) => s.catalog.codegen);
  const languages = useMemo<LanguageInfo[]>(
    () => [...builtin, ...pluginTargets.map((t) => ({ id: t.id as CodegenLanguage, label: t.name, group: 'Plugins', editor: t.language, extension: 'txt' }))],
    [builtin, pluginTargets],
  );
  const [lang, setLang] = useState<CodegenLanguage>(savedLanguage);
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || languagesCache) return;
    importApi
      .codegenLanguages()
      .then((l) => {
        languagesCache = l;
        setLanguages(l);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [open]);

  useEffect(() => {
    if (open) setIncludeSecrets(false);
  }, [open]);

  const requestKey = JSON.stringify(request);
  useEffect(() => {
    if (!open || !project) return;
    let stale = false;
    setBusy(true);
    setError(null);
    (isPluginOp(lang) ? importApi.codegenPlugin(project.root, path, request, envId, lang, includeSecrets) : importApi.codegen(project.root, path, request, envId, lang, includeSecrets))
      .then((c) => !stale && setCode(c))
      .catch((e) => {
        if (stale) return;
        setCode('');
        setError(errorMessage(e));
      })
      .finally(() => !stale && setBusy(false));
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, project, path, requestKey, envId, lang, includeSecrets]);

  const groups = useMemo(() => {
    const out: { group: string; items: LanguageInfo[] }[] = [];
    for (const l of languages) {
      const g = out.find((x) => x.group === l.group);
      if (g) g.items.push(l);
      else out.push({ group: l.group, items: [l] });
    }
    return out;
  }, [languages]);

  const current = languages.find((l) => l.id === lang);
  const editorLang: CodeLanguage = current && (EDITOR_LANGS as string[]).includes(current.editor) ? (current.editor as CodeLanguage) : 'plaintext';

  const pick = (id: CodegenLanguage) => {
    setLang(id);
    try {
      localStorage.setItem(LANG_KEY, id);
    } catch {
      /* storage unavailable */
    }
  };

  return (
    <Modal open={open} onClose={onClose} title={`Generate code — ${request.name || 'request'}`} size="xl" bodyClassName="codegen-body">
      <div className="codegen">
        <div className="codegen-langs" role="listbox" aria-label="Language">
          {groups.map((g) => (
            <div key={g.group}>
              <div className="list-group-header">{g.group}</div>
              {g.items.map((l) => (
                <div
                  key={l.id}
                  className={clsx('list-item', l.id === lang && 'on')}
                  role="option"
                  aria-selected={l.id === lang}
                  tabIndex={0}
                  onClick={() => pick(l.id)}
                  onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && pick(l.id)}
                >
                  <span className="truncate">{l.label}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
        <div className="codegen-main">
          <div className="codegen-toolbar">
            <span className="strong">{current?.label ?? lang}</span>
            {busy && <Spinner />}
            <span className="grow" />
            <Switch checked={includeSecrets} onChange={setIncludeSecrets} label="Include secrets" title="Off: secret values are replaced with ••••••" />
            <CopyButton text={code} label="Copy" />
          </div>
          {error && (
            <div className="p-4">
              <Callout kind="danger">{error}</Callout>
            </div>
          )}
          {includeSecrets && !error && (
            <div className="px-5" style={{ paddingTop: 'var(--space-3)' }}>
              <Callout kind="warning">This snippet contains secret values — don't paste it into tickets, chats or repositories.</Callout>
            </div>
          )}
          <div className="codegen-code">
            <CodeEditor value={code} language={editorLang} readOnly path={`codegen.${current?.extension ?? 'txt'}`} />
          </div>
        </div>
      </div>
    </Modal>
  );
}
