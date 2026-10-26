import { MarkdownField } from '@/components/markdown/MarkdownEditor';
import { useProjectLinkOpener } from '@/features/notes/NotesPage';

/** The "Docs" tab of requests and realtime connections: Markdown notes saved in the file. */
export function DocsField({ value, onChange }: { value: string | undefined; onChange(v: string | undefined): void }) {
  const open = useProjectLinkOpener();
  return (
    <MarkdownField
      value={value ?? ''}
      onChange={(v) => onChange(v || undefined)}
      minHeight={240}
      placeholder="Notes for this file — Markdown: / for blocks, [[ links to requests, notes, to-dos…"
      onProjectLink={(href) => open(href)}
      ariaLabel="Notes"
    />
  );
}
