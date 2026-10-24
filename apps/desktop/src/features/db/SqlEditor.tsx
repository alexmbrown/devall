import { useEffect, useRef } from 'react';
import Editor, { type OnMount } from '@monaco-editor/react';
import { currentMonacoTheme, monaco } from '@/components/editor/monaco';
import { Spinner } from '@/components/ui/misc';
import { usePrefs } from '@/store/prefs';
import { dbApi, type Dialect, type SchemaTree, type Statement } from '@/lib/api/db';

/** Schema per editor model, read by the (single, global) SQL completion provider. */
const schemas = new Map<string, SchemaTree>();
let providerRegistered = false;

function registerCompletions() {
  if (providerRegistered) return;
  providerRegistered = true;
  monaco.languages.registerCompletionItemProvider('sql', {
    triggerCharacters: ['.', ' '],
    provideCompletionItems(model, position) {
      const tree = schemas.get(model.uri.toString());
      if (!tree) return { suggestions: [] };
      const word = model.getWordUntilPosition(position);
      const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
      const before = model.getValueInRange({ startLineNumber: position.lineNumber, startColumn: 1, endLineNumber: position.lineNumber, endColumn: word.startColumn });
      const qualifier = /([A-Za-z_][\w$]*)\.$/.exec(before)?.[1]?.toLowerCase();
      const K = monaco.languages.CompletionItemKind;
      const out: monaco.languages.CompletionItem[] = [];
      const seen = new Set<string>();
      const add = (label: string, kind: monaco.languages.CompletionItemKind, detail: string, sort: string) => {
        const key = `${kind}|${label}`;
        if (seen.has(key)) return;
        seen.add(key);
        out.push({ label, kind, detail, insertText: /^[a-z_][a-z0-9_$]*$/i.test(label) ? label : `"${label}"`, range, sortText: sort + label });
      };
      for (const s of tree.schemas) {
        if (qualifier && s.name.toLowerCase() === qualifier) {
          for (const t of s.tables) add(t.name, t.kind === 'table' ? K.Struct : K.Interface, `${t.kind} in ${s.name}`, '0');
          continue;
        }
        for (const t of s.tables) {
          if (qualifier && t.name.toLowerCase() === qualifier) {
            for (const c of t.columns) add(c.name, K.Field, `${c.dataType}${c.primaryKey ? ' · PK' : ''}`, '0');
            continue;
          }
          if (!qualifier) {
            add(t.name, t.kind === 'table' ? K.Struct : K.Interface, `${t.kind}${s.name === tree.defaultSchema ? '' : ` in ${s.name}`}`, s.name === tree.defaultSchema ? '1' : '2');
            for (const c of t.columns) add(c.name, K.Field, `${t.name}.${c.name} ${c.dataType}`, '3');
          }
        }
        if (!qualifier) add(s.name, K.Module, 'schema', '4');
      }
      return { suggestions: out };
    },
  });
}

/** The statement containing `offset` (or the one just before it). */
export function statementAt(stmts: Statement[], offset: number): Statement | undefined {
  return stmts.find((s) => offset >= s.from && offset <= s.to) ?? [...stmts].reverse().find((s) => s.to <= offset) ?? stmts[0];
}

interface Props {
  value: string;
  onChange(v: string): void;
  dialect: Dialect;
  /** Unique model path (keeps undo history per tab). */
  path: string;
  schema?: SchemaTree;
  /** `all`: whole script; otherwise the selection or the statement under the cursor. */
  onRun(sql: string, all: boolean): void;
  onExplain(sql: string): void;
  onSave?(): void;
  onCancel?(): void;
}

/** Monaco SQL editor: Ctrl+Enter runs the selection or the statement under the cursor,
 * Ctrl+Shift+Enter runs everything, Ctrl+E explains, Ctrl+S saves, Esc cancels. */
export function SqlEditor({ value, onChange, dialect, path, schema, onRun, onExplain, onSave, onCancel }: Props) {
  const wordWrap = usePrefs((s) => s.wordWrap);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const handlers = useRef({ onRun, onExplain, onSave, onCancel, dialect });
  handlers.current = { onRun, onExplain, onSave, onCancel, dialect };
  const decorations = useRef<monaco.editor.IEditorDecorationsCollection | null>(null);

  useEffect(() => {
    registerCompletions();
  }, []);

  useEffect(() => {
    const uri = monaco.Uri.parse(`inmemory://db/${encodeURIComponent(path)}.sql`).toString();
    if (schema) schemas.set(uri, schema);
    else schemas.delete(uri);
  }, [schema, path]);

  /** Selection, or the statement under the cursor (flashed). */
  const target = async (): Promise<string | null> => {
    const ed = editorRef.current;
    const model = ed?.getModel();
    if (!ed || !model) return null;
    const sel = ed.getSelection();
    if (sel && !sel.isEmpty()) return model.getValueInRange(sel);
    const text = model.getValue();
    const offset = model.getOffsetAt(ed.getPosition() ?? { lineNumber: 1, column: 1 });
    const stmts = await dbApi.split(text, handlers.current.dialect);
    const st = statementAt(stmts, offset);
    if (!st) return null;
    const a = model.getPositionAt(st.from);
    const b = model.getPositionAt(st.to);
    decorations.current?.clear();
    decorations.current = ed.createDecorationsCollection([
      { range: new monaco.Range(a.lineNumber, a.column, b.lineNumber, b.column), options: { className: 'db-run-flash', isWholeLine: false } },
    ]);
    setTimeout(() => decorations.current?.clear(), 700);
    return st.text;
  };

  const onMount: OnMount = (editor) => {
    editorRef.current = editor;
    const K = monaco.KeyMod;
    const C = monaco.KeyCode;
    editor.addCommand(K.CtrlCmd | C.Enter, () => {
      void target().then((sql) => sql && handlers.current.onRun(sql, false));
    });
    editor.addCommand(K.CtrlCmd | K.Shift | C.Enter, () => handlers.current.onRun(editor.getModel()?.getValue() ?? '', true));
    editor.addCommand(K.CtrlCmd | C.KeyE, () => {
      void target().then((sql) => sql && handlers.current.onExplain(sql));
    });
    editor.addCommand(K.CtrlCmd | C.KeyS, () => handlers.current.onSave?.());
    editor.addAction({
      id: 'devall.db.cancel',
      label: 'Cancel running query',
      keybindings: [C.Escape],
      precondition: '!suggestWidgetVisible && !findWidgetVisible',
      run: () => handlers.current.onCancel?.(),
    });
    editor.addAction({
      id: 'devall.db.runAll',
      label: 'Run all statements',
      contextMenuGroupId: 'navigation',
      contextMenuOrder: 1.6,
      run: () => handlers.current.onRun(editor.getModel()?.getValue() ?? '', true),
    });
    editor.addAction({
      id: 'devall.db.run',
      label: 'Run statement / selection',
      contextMenuGroupId: 'navigation',
      contextMenuOrder: 1.5,
      run: () => {
        void target().then((sql) => sql && handlers.current.onRun(sql, false));
      },
    });
    editor.addAction({
      id: 'devall.db.explain',
      label: 'Explain statement',
      contextMenuGroupId: 'navigation',
      contextMenuOrder: 1.7,
      run: () => {
        void target().then((sql) => sql && handlers.current.onExplain(sql));
      },
    });
    editor.focus();
  };

  return (
    <div className="h-full" style={{ minHeight: 0 }}>
      <Editor
        value={value}
        language="sql"
        path={`inmemory://db/${encodeURIComponent(path)}.sql`}
        theme={currentMonacoTheme()}
        onChange={(v) => onChange(v ?? '')}
        onMount={onMount}
        loading={<Spinner />}
        options={{
          minimap: { enabled: false },
          fontSize: 12,
          fontFamily: "ui-monospace, 'JetBrains Mono', 'Cascadia Code', Menlo, Consolas, monospace",
          lineHeight: 18,
          scrollBeyondLastLine: false,
          automaticLayout: true,
          tabSize: 2,
          wordWrap: wordWrap ? 'on' : 'off',
          padding: { top: 6, bottom: 6 },
          scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10, alwaysConsumeMouseWheel: false },
          fixedOverflowWidgets: true,
          quickSuggestions: { strings: false, other: true, comments: false },
          stickyScroll: { enabled: false },
          lineNumbersMinChars: 3,
        }}
      />
    </div>
  );
}
