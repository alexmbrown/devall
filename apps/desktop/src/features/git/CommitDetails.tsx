import { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, Copy, GitCommitHorizontal, GitCompare } from 'lucide-react';
import type { monaco } from '@/components/editor/monaco';
import { CopyButton, Empty, RelTime, Segmented, Spinner } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type CommitInfo, type RangeFiles } from '@/lib/api/git';
import { ChangedFileList, Counts, StatusLetter } from './ChangedFileList';
import { useCompare } from './compareStore';
import { DiffPane } from './DiffPane';
import { displayOrder } from './review';
import { fileViewItems } from './BlameView';

/**
 * The selected commit in the graph: message, author, parents and refs, its files (against its
 * first parent; a root commit against the empty tree) and the selected file's diff.
 */
export function CommitDetails({ root, repo, sha, onJump, onCompareParent }: { root: string; repo: string | null; sha: string; onJump(hash: string): void; onCompareParent(parent: string, sha: string): void }) {
  const [info, setInfo] = useState<CommitInfo | null>(null);
  const [range, setRange] = useState<RangeFiles | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [tree, setTree] = useState(false);
  const [showGen, setShowGen] = useState(false);
  const opts = useCompare((s) => s.opts);
  const setOpts = useCompare((s) => s.setOpts);
  const editor = useRef<monaco.editor.IStandaloneDiffEditor | null>(null);
  const onEditor = useCallback((ed: monaco.editor.IStandaloneDiffEditor | null) => (editor.current = ed), []);

  useEffect(() => {
    let alive = true;
    setInfo(null);
    setRange(null);
    setError(null);
    Promise.all([gitApi.commitInfo(root, repo, sha), gitApi.commitRange(root, repo, sha, sha)])
      .then(([i, r]) => {
        if (!alive) return;
        setInfo(i);
        setRange(r);
        setSel((cur) => (cur && r.files.some((f) => f.path === cur) ? cur : (displayOrder(r.files, false).main[0]?.path ?? r.files[0]?.path ?? null)));
      })
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [root, repo, sha]);

  if (error) return <Empty icon={<GitCommitHorizontal />} title="Couldn't read the commit">{error}</Empty>;
  if (!info || !range) return <Empty icon={<Spinner />} title="Reading the commit…" />;
  const file = range.files.find((f) => f.path === sel) ?? null;
  const order = displayOrder(range.files, tree);
  return (
    <div className="git-details">
      <div className="git-details-side">
        <div className="git-details-head">
          <div className="strong small git-details-subject">{info.subject}</div>
          {info.body && <pre className="git-details-body">{info.body}</pre>}
          <div className="xs subtle">
            {info.author} &lt;{info.email}&gt; · <RelTime ts={info.date} />
            {info.committer !== info.author && <> · committed by {info.committer}</>}
          </div>
          <div className="xs row-sm wrap">
            <span className="mono">{info.hash.slice(0, 12)}</span>
            <CopyButton text={info.hash} title="Copy SHA" />
            {info.parents.map((p) => (
              <button key={p} className="btn btn-sm btn-ghost mono" title="Go to parent" onClick={() => onJump(p)}>
                ↳ {p.slice(0, 7)}
              </button>
            ))}
            {info.parents.length > 1 && <span className="badge outline">merge — files vs first parent</span>}
          </div>
          <div className="row-sm xs">
            <Segmented value={tree ? 'tree' : 'list'} onChange={(v) => setTree(v === 'tree')} options={[{ value: 'list', label: 'List' }, { value: 'tree', label: 'Tree' }]} />
            <span className="subtle grow">
              {range.files.length} file{range.files.length === 1 ? '' : 's'}
            </span>
            {info.parents[0] && (
              <button className="btn btn-sm btn-ghost" title="Open this commit in the compare view" onClick={() => onCompareParent(info.parents[0], info.hash)}>
                <GitCompare /> Review
              </button>
            )}
          </div>
        </div>
        <ChangedFileList main={order.main} generated={order.generated} tree={tree} selected={sel} onSelect={setSel} showGenerated={showGen} onToggleGenerated={setShowGen} menu={(f) => [{ heading: f.path }, ...fileViewItems(f.path, sha, f.status !== 'D')]} />
      </div>
      <div className="git-details-diff">
        {file ? (
          <>
            <div className="toolbar git-diff-toolbar">
              <StatusLetter status={file.status} />
              <span className="mono xs truncate grow">{file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}</span>
              <Counts f={file} />
              <Segmented value={opts.layout} onChange={(layout) => setOpts({ layout })} options={[{ value: 'split', label: 'Split' }, { value: 'unified', label: 'Unified' }]} />
              <button className="icon-btn sm" title="Previous change" onClick={() => editor.current?.goToDiff('previous')}>
                <ChevronUp />
              </button>
              <button className="icon-btn sm" title="Next change" onClick={() => editor.current?.goToDiff('next')}>
                <ChevronDown />
              </button>
              <CopyButton text={file.path} title="Copy path" />
            </div>
            <div className="grow min-h-0 git-review-diff">
              <DiffPane
                root={root}
                repo={repo}
                old={file.status === 'A' ? { type: 'empty' } : { type: 'rev', rev: range.from, path: file.oldPath ?? file.path }}
                next={file.status === 'D' ? { type: 'empty' } : { type: 'rev', rev: range.to, path: file.path }}
                path={file.path}
                lines={(file.added ?? 0) + (file.deleted ?? 0)}
                opts={opts}
                onEditor={onEditor}
              />
            </div>
          </>
        ) : (
          <Empty icon={<Copy />} title={range.files.length ? 'Select a file' : 'No file changes'} />
        )}
      </div>
    </div>
  );
}
