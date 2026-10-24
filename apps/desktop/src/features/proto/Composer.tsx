import { useState, type ReactNode } from 'react';
import { Bookmark, BookmarkPlus, Pencil, Send, Trash2, Wand2 } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Segmented } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import type { MessageFormat } from '@/lib/api/proto';

const FORMAT_LABEL: Record<MessageFormat, string> = { text: 'Text', json: 'JSON', 'binary-base64': 'Binary (base64)' };

export interface ComposerProps {
  value: string;
  onChange(v: string): void;
  /** Omit to hide the format switch (e.g. gRPC is always JSON). */
  format?: MessageFormat;
  onFormat?(f: MessageFormat): void;
  formats?: MessageFormat[];
  onSend(): void;
  canSend: boolean;
  sendLabel?: string;
  sendTitle?: string;
  /** Saved messages (names only; the editor maps them to its own preset type). */
  presets: { name: string }[];
  onLoadPreset(i: number): void;
  onSavePreset(name: string): void;
  onUpdatePreset(i: number): void;
  onDeletePreset(i: number): void;
  /** Fields above the editor (topic/QoS, event name, …). */
  fields?: ReactNode;
  /** Extra buttons next to Send. */
  actions?: ReactNode;
  /** Monaco model path (unique per file). */
  modelPath: string;
  hint?: ReactNode;
}

/** Message editor with a format switch, JSON formatting, saved presets and a send button. */
export function Composer(p: ComposerProps) {
  const [loaded, setLoaded] = useState<number | null>(null);
  const format = p.format ?? 'json';
  const current = loaded !== null && loaded < p.presets.length ? loaded : null;

  const presetItems = (): MenuItemDef[] => {
    const items: MenuItemDef[] = p.presets.length
      ? [
          { heading: 'Load a saved message' },
          ...p.presets.map((x, i) => ({
            label: x.name || `Message ${i + 1}`,
            icon: <Bookmark />,
            onSelect: () => {
              setLoaded(i);
              p.onLoadPreset(i);
            },
          })),
        ]
      : [{ label: 'No saved messages yet', disabled: true }];
    items.push({ separator: true });
    items.push({
      label: 'Save as new…',
      icon: <BookmarkPlus />,
      onSelect: async () => {
        const name = await promptDialog({ title: 'Save message', label: 'Name', initial: current !== null ? `${p.presets[current].name} copy` : '', confirmText: 'Save' });
        if (name?.trim()) {
          p.onSavePreset(name.trim());
          setLoaded(p.presets.length);
        }
      },
    });
    if (current !== null) {
      const name = p.presets[current].name;
      items.push({ label: `Update “${name}”`, icon: <Pencil />, onSelect: () => p.onUpdatePreset(current) });
      items.push({
        label: `Delete “${name}”`,
        icon: <Trash2 />,
        danger: true,
        onSelect: async () => {
          if (await confirmDialog({ title: `Delete “${name}”?`, message: 'The saved message is removed from the file.', confirmText: 'Delete', danger: true })) {
            p.onDeletePreset(current);
            setLoaded(null);
          }
        },
      });
    }
    return items;
  };

  return (
    <div className="proto-composer">
      {p.fields && <div className="proto-fields">{p.fields}</div>}
      <div className="row-sm">
        {p.format !== undefined && p.onFormat && (
          <Segmented value={format} onChange={(f) => p.onFormat!(f)} options={(p.formats ?? (['text', 'json', 'binary-base64'] as MessageFormat[])).map((f) => ({ value: f, label: FORMAT_LABEL[f] }))} />
        )}
        {format === 'json' && (
          <button
            className="btn btn-sm btn-ghost"
            title="Pretty-print (templates like {{id}} must be inside strings)"
            onClick={() => {
              try {
                p.onChange(JSON.stringify(JSON.parse(p.value), null, 2));
              } catch (e) {
                toast.error('Not valid JSON', (e as Error).message);
              }
            }}
          >
            <Wand2 /> Format
          </button>
        )}
        <Menu
          items={presetItems}
          trigger={(t) => (
            <button className="btn btn-sm btn-ghost" {...t} title="Saved messages (stored in the file)">
              <Bookmark /> {current !== null ? p.presets[current].name : `Saved (${p.presets.length})`}
            </button>
          )}
        />
        <span className="spacer" />
        {p.hint && <span className="subtle xs">{p.hint}</span>}
        {p.actions}
        <button className="btn btn-primary btn-sm" onClick={p.onSend} disabled={!p.canSend} title={p.sendTitle ?? 'Send (Ctrl+Enter)'}>
          <Send /> {p.sendLabel ?? 'Send'}
        </button>
      </div>
      <div className="editor-shell">
        <CodeEditor value={p.value} onChange={p.onChange} language={format === 'json' ? 'json' : 'plaintext'} path={p.modelPath} minimal />
      </div>
    </div>
  );
}
