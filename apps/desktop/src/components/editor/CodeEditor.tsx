import { useEffect, useRef } from 'react';
import Editor, { DiffEditor, type OnMount } from '@monaco-editor/react';
import { currentMonacoTheme, monaco } from './monaco';
import { usePrefs } from '@/store/prefs';
import { Spinner } from '@/components/ui/misc';

export type CodeLanguage = 'json' | 'xml' | 'html' | 'handlebars' | 'plaintext' | 'javascript' | 'typescript' | 'java' | 'shell' | 'yaml' | 'css' | 'kotlin' | 'markdown' | 'graphql' | 'sql' | 'python' | 'go' | 'rust' | 'csharp' | 'php' | 'ruby' | 'swift' | 'powershell' | 'ini';

interface CodeEditorProps {
  value: string;
  onChange?(v: string): void;
  language?: CodeLanguage;
  readOnly?: boolean;
  /** Model URI path; drives which JSON schema applies (see monaco.ts). */
  path?: string;
  height?: number | string;
  minimal?: boolean;
  onSave?(): void;
  onValidate?(markers: monaco.editor.IMarker[]): void;
  lineNumbers?: boolean;
  autoHeight?: { min: number; max: number };
}

const BASE_OPTIONS: monaco.editor.IStandaloneEditorConstructionOptions = {
  minimap: { enabled: false },
  fontSize: 12,
  fontFamily: "ui-monospace, 'JetBrains Mono', 'Cascadia Code', Menlo, Consolas, monospace",
  lineHeight: 18,
  scrollBeyondLastLine: false,
  automaticLayout: true,
  tabSize: 2,
  renderLineHighlight: 'line',
  folding: true,
  padding: { top: 6, bottom: 6 },
  scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10, alwaysConsumeMouseWheel: false },
  fixedOverflowWidgets: true,
  formatOnPaste: true,
  quickSuggestions: { strings: true, other: true, comments: false },
  stickyScroll: { enabled: false },
};

export function CodeEditor({
  value,
  onChange,
  language = 'json',
  readOnly,
  path,
  height = '100%',
  minimal,
  onSave,
  onValidate,
  lineNumbers = true,
  autoHeight,
}: CodeEditorProps) {
  const wordWrap = usePrefs((s) => s.wordWrap);
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const hostRef = useRef<HTMLDivElement>(null);

  const onMount: OnMount = (editor) => {
    editorRef.current = editor;
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => saveRef.current?.());
    if (autoHeight && hostRef.current) {
      const host = hostRef.current;
      const update = () => {
        const h = Math.min(autoHeight.max, Math.max(autoHeight.min, editor.getContentHeight()));
        host.style.height = `${h}px`;
        editor.layout();
      };
      editor.onDidContentSizeChange(update);
      update();
    }
  };

  useEffect(() => () => void (editorRef.current = null), []);

  return (
    <div ref={hostRef} style={{ height: autoHeight ? autoHeight.min : height, minHeight: 0 }} className="h-full">
      <Editor
        value={value}
        language={language}
        path={path}
        theme={currentMonacoTheme()}
        onChange={(v) => onChange?.(v ?? '')}
        onMount={onMount}
        onValidate={onValidate}
        loading={<Spinner />}
        options={{
          ...BASE_OPTIONS,
          readOnly,
          wordWrap: wordWrap ? 'on' : 'off',
          lineNumbers: lineNumbers && !minimal ? 'on' : 'off',
          glyphMargin: false,
          folding: !minimal,
          lineDecorationsWidth: minimal ? 6 : 8,
          lineNumbersMinChars: minimal ? 0 : 3,
          renderLineHighlight: minimal ? 'none' : 'line',
          domReadOnly: readOnly,
        }}
      />
    </div>
  );
}

export function CodeDiff({ original, modified, language = 'json', height = '100%' }: { original: string; modified: string; language?: CodeLanguage; height?: number | string }) {
  const wordWrap = usePrefs((s) => s.wordWrap);
  const diffRef = useRef<monaco.editor.IStandaloneDiffEditor | null>(null);
  // @monaco-editor/react disposes the text models before the diff widget lets go of them, which
  // throws "TextModel got disposed before DiffEditorWidget model got reset". Keep the models and
  // detach + dispose them ourselves in the right order.
  useEffect(
    () => () => {
      const ed = diffRef.current;
      const models = ed?.getModel();
      ed?.setModel(null);
      models?.original.dispose();
      models?.modified.dispose();
    },
    [],
  );
  return (
    <div style={{ height, minHeight: 0 }}>
      <DiffEditor
        original={original}
        modified={modified}
        language={language}
        theme={currentMonacoTheme()}
        loading={<Spinner />}
        keepCurrentOriginalModel
        keepCurrentModifiedModel
        onMount={(ed) => (diffRef.current = ed)}
        options={{
          ...BASE_OPTIONS,
          readOnly: true,
          renderSideBySide: true,
          wordWrap: wordWrap ? 'on' : 'off',
          originalEditable: false,
          diffWordWrap: wordWrap ? 'on' : 'off',
          hideUnchangedRegions: { enabled: true, minimumLineCount: 3, contextLineCount: 2 },
        }}
      />
    </div>
  );
}

/** Pick a Monaco language from a content-type or filename. */
export function languageFor(contentTypeOrName?: string, body?: string): CodeLanguage {
  const s = (contentTypeOrName ?? '').toLowerCase();
  if (s.includes('json') || s.endsWith('.json')) return 'json';
  if (s.includes('xml') || s.endsWith('.xml') || s.includes('soap')) return 'xml';
  if (s.includes('html') || s.endsWith('.html') || s.endsWith('.htm')) return 'html';
  if (s.endsWith('.hbs') || s.endsWith('.handlebars')) return 'handlebars';
  if (s.includes('javascript') || s.endsWith('.js')) return 'javascript';
  if (s.includes('yaml') || s.endsWith('.yml') || s.endsWith('.yaml')) return 'yaml';
  if (s.includes('css') || s.endsWith('.css')) return 'css';
  const t = body?.trimStart() ?? '';
  if (t.startsWith('{') || t.startsWith('[')) return 'json';
  if (t.startsWith('<')) return 'xml';
  if (t.includes('{{')) return 'handlebars';
  return 'plaintext';
}
