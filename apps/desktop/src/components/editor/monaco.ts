// Bundles Monaco locally (no CDN) so the app works offline and on locked-down networks.
import * as monaco from 'monaco-editor';
import { loader } from '@monaco-editor/react';
import EditorWorker from 'monaco-editor/editor/editor.worker?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker?worker';
import TsWorker from 'monaco-editor/language/typescript/ts.worker?worker';

declare global {
  interface Window {
    MonacoEnvironment?: { getWorker(id: string, label: string): Worker };
  }
}

self.MonacoEnvironment = {
  getWorker(_id, label) {
    if (label === 'json') return new JsonWorker();
    if (label === 'typescript' || label === 'javascript') return new TsWorker();
    return new EditorWorker();
  },
};

let configured = false;
export function configureMonaco() {
  if (configured) return;
  configured = true;
  // Request bodies may contain {{templates}} that aren't strictly valid JSON; keep validation
  // on but tolerate comments and trailing commas (common in hand-written payloads).
  monaco.json.jsonDefaults.setDiagnosticsOptions({ validate: true, allowComments: true, trailingCommas: 'ignore', enableSchemaRequest: false });

  const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const defineThemes = () => {
    const bg = css('--bg-code') || '#ffffff';
    monaco.editor.defineTheme('devall-light', {
      base: 'vs',
      inherit: true,
      rules: [],
      colors: { 'editor.background': bg, 'editorGutter.background': bg, 'editor.lineHighlightBackground': '#00000008' },
    });
    monaco.editor.defineTheme('devall-dark', {
      base: 'vs-dark',
      inherit: true,
      rules: [],
      colors: { 'editor.background': bg, 'editorGutter.background': bg, 'editor.lineHighlightBackground': '#ffffff08' },
    });
  };
  defineThemes();
  new MutationObserver(() => {
    defineThemes();
    monaco.editor.setTheme(currentMonacoTheme());
  }).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
}

loader.config({ monaco });
configureMonaco();

export function currentMonacoTheme() {
  return document.documentElement.dataset.theme === 'dark' ? 'devall-dark' : 'devall-light';
}

export { monaco };
