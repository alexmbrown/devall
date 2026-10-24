import { monaco } from '@/components/editor/monaco';
import { API_TREE, type ApiMember } from './apiReference';

/** Monaco model paths of script editors start with this; completions only apply there. */
export const SCRIPT_MODEL_PREFIX = 'devall-script';

let registered = false;

/**
 * `pm.` / `pm.response.to.have.` … member completions for script editors. Runs on the main thread
 * (no TypeScript worker needed) and only for models created by `ScriptsEditor`.
 */
export function registerScriptCompletions() {
  if (registered) return;
  registered = true;
  const { CompletionItemKind, CompletionItemInsertTextRule } = monaco.languages;
  const kindOf = (k: ApiMember['kind']) => (k === 'method' ? CompletionItemKind.Method : k === 'property' ? CompletionItemKind.Property : CompletionItemKind.Module);

  monaco.languages.registerCompletionItemProvider('javascript', {
    triggerCharacters: ['.'],
    provideCompletionItems(model, position) {
      if (!model.uri.path.replace(/^\//, '').startsWith(SCRIPT_MODEL_PREFIX)) return { suggestions: [] };
      const line = model.getLineContent(position.lineNumber).slice(0, position.column - 1);
      // `pm.response.to.ha|` → object path `pm.response.to`, partial word `ha`.
      const m = /((?:[A-Za-z_$][\w$]*\.)*)([A-Za-z_$][\w$]*)?$/.exec(line);
      if (!m) return { suggestions: [] };
      const path = m[1].replace(/\.$/, '');
      const members = API_TREE[path];
      if (!members) return { suggestions: [] };
      const word = model.getWordUntilPosition(position);
      const range = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
      return {
        suggestions: members.map((x, i) => ({
          label: x.name,
          kind: kindOf(x.kind),
          detail: path ? `${path}.${x.name}` : x.name,
          documentation: x.doc,
          insertText: x.kind === 'method' ? (x.insert ?? `${x.name}($0)`) : x.name,
          insertTextRules: x.kind === 'method' ? CompletionItemInsertTextRule.InsertAsSnippet : undefined,
          sortText: String(i).padStart(3, '0'),
          range,
        })),
      };
    },
  });
}
