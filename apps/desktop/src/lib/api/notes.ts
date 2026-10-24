import { invoke } from '@tauri-apps/api/core';

export type NoteNode =
  | { kind: 'folder'; name: string; path: string; children: NoteNode[] }
  | { kind: 'note'; title: string; path: string; tags: string[]; modified: number; format: 'markdown' | 'notebook' };

export interface NoteHit {
  path: string;
  title: string;
  snippet: string;
  matches: number;
}

export interface ContextDoc {
  kind: 'note' | 'todos' | 'requests' | 'environments';
  source: string;
  title: string;
  text: string;
}

export const notesApi = {
  tree: (root: string) => invoke<NoteNode>('notes_tree', { root }),
  read: (root: string, path: string) => invoke<string>('notes_read', { root, path }),
  save: (root: string, path: string, content: string) => invoke<void>('notes_save', { root, path, content }),
  create: (root: string, folder: string, title: string) => invoke<string>('notes_create', { root, folder, title }),
  createNotebook: (root: string, folder: string, title: string) => invoke<string>('notes_create_notebook', { root, folder, title }),
  createFolder: (root: string, parent: string, name: string) => invoke<string>('notes_create_folder', { root, parent, name }),
  rename: (root: string, path: string, name: string) => invoke<string>('notes_rename', { root, path, name }),
  remove: (root: string, path: string) => invoke<void>('notes_delete', { root, path }),
  search: (root: string, query: string) => invoke<NoteHit[]>('notes_search', { root, query }),
};

export const contextApi = {
  docs: (root: string, query?: string, limit?: number) => invoke<ContextDoc[]>('context_docs', { root, query, limit }),
};
