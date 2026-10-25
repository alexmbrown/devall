import { useEffect, useRef, useState } from 'react';
import { FileWarning, ImageIcon } from 'lucide-react';
import { CodeDiff, languageFor, type CodeLanguage } from '@/components/editor/CodeEditor';
import type { monaco } from '@/components/editor/monaco';
import { Empty, Spinner, formatBytes } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type DiffSide, type FileDiff } from '@/lib/api/git';

/** Diffs with more changed lines than this wait for "Load anyway". */
export const MAX_DIFF_LINES = 5000;

const EXT: Record<string, CodeLanguage> = {
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  rs: 'rust',
  py: 'python',
  go: 'go',
  java: 'java',
  kt: 'kotlin',
  kts: 'kotlin',
  cs: 'csharp',
  php: 'php',
  rb: 'ruby',
  swift: 'swift',
  ps1: 'powershell',
  psm1: 'powershell',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  yml: 'yaml',
  yaml: 'yaml',
  json: 'json',
  jsonc: 'json',
  md: 'markdown',
  css: 'css',
  scss: 'css',
  less: 'css',
  html: 'html',
  htm: 'html',
  xml: 'xml',
  svg: 'xml',
  sql: 'sql',
  graphql: 'graphql',
  gql: 'graphql',
  toml: 'ini',
  ini: 'ini',
  cfg: 'ini',
  hbs: 'handlebars',
};

/** Syntax highlighting for a path in a diff. */
export function diffLanguage(path: string, body?: string): CodeLanguage {
  const ext = path.split('/').pop()?.split('.').pop()?.toLowerCase() ?? '';
  return EXT[ext] ?? languageFor(path, body);
}

export interface DiffViewOpts {
  layout: 'split' | 'unified';
  ignoreWs: boolean;
  wrap: boolean;
  full: boolean;
}

/**
 * One file's diff between two sides (any revision, blob, the index or the working tree), loaded
 * lazily: answers for a file that is no longer selected are dropped; diffs over
 * {@link MAX_DIFF_LINES} changed lines or over the size cap wait for "Load anyway". Images show side
 * by side; other binary files say so.
 */
export function DiffPane({
  root,
  repo,
  old,
  next,
  path,
  lines,
  opts,
  onEditor,
}: {
  root: string;
  repo: string | null;
  old: DiffSide;
  next: DiffSide;
  path: string;
  /** Added + deleted lines when known (for the size cap). */
  lines?: number | null;
  opts: DiffViewOpts;
  onEditor?(ed: monaco.editor.IStandaloneDiffEditor | null): void;
}) {
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [force, setForce] = useState(false);
  const key = `${repo}|${JSON.stringify(old)}|${JSON.stringify(next)}|${path}`;
  const token = useRef(0);
  const big = (lines ?? 0) > MAX_DIFF_LINES && !force;

  useEffect(() => setForce(false), [key]);
  useEffect(() => {
    const t = ++token.current;
    setDiff(null);
    setError(null);
    onEditor?.(null); // the editor of the previous file is going away
    if (big) return;
    gitApi
      .fileDiff(root, repo, old, next, path, force)
      .then((d) => t === token.current && setDiff(d))
      .catch((e) => t === token.current && setError(errorMessage(e)));
    // `key` stands for old/next/path (objects that are new on every render).
  }, [root, key, force, big]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => onEditor?.(null), [onEditor]);

  if (big)
    return (
      <Empty icon={<FileWarning />} title="Large diff" action={<button className="btn btn-sm" onClick={() => setForce(true)}>Load anyway</button>}>
        {lines?.toLocaleString()} changed lines — not loaded automatically.
      </Empty>
    );
  if (error) return <Empty icon={<FileWarning />} title="Couldn't load the diff">{error}</Empty>;
  if (!diff) return <Empty icon={<Spinner />} title="Loading diff…" />;
  if (diff.tooLarge && !force)
    return (
      <Empty icon={<FileWarning />} title="File too large" action={<button className="btn btn-sm" onClick={() => setForce(true)}>Load anyway</button>}>
        {formatBytes(Math.max(diff.oldSize, diff.newSize))} — not loaded automatically.
      </Empty>
    );
  if (diff.tooLarge) return <Empty icon={<FileWarning />} title="File too large to show">{formatBytes(Math.max(diff.oldSize, diff.newSize))}</Empty>;
  if (diff.image) return <ImageDiff image={diff.image} oldSize={diff.oldSize} newSize={diff.newSize} />;
  if (diff.binary) return <Empty icon={<FileWarning />} title="Binary file">{formatBytes(diff.oldSize)} → {formatBytes(diff.newSize)}</Empty>;
  return (
    <CodeDiff
      key={path}
      original={diff.original}
      modified={diff.modified}
      language={diffLanguage(path, diff.modified || diff.original)}
      sideBySide={opts.layout === 'split'}
      ignoreWhitespace={opts.ignoreWs}
      wrap={opts.wrap}
      hideUnchanged={!opts.full}
      onMount={onEditor}
    />
  );
}

function ImageDiff({ image, oldSize, newSize }: { image: NonNullable<FileDiff['image']>; oldSize: number; newSize: number }) {
  const side = (label: string, src: string | null, size: number) => (
    <div className="git-image-side">
      <div className="xs subtle">
        {label}
        {src ? ` · ${formatBytes(size)}` : ''}
      </div>
      <div className="git-image-frame">{src ? <img src={src} alt={label} /> : <span className="subtle small row-sm"><ImageIcon size={14} /> none</span>}</div>
    </div>
  );
  return (
    <div className="git-image-diff">
      {side('Before', image.old, oldSize)}
      {side('After', image.new, newSize)}
    </div>
  );
}
