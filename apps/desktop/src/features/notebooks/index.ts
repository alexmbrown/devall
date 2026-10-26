// Runnable notebooks: `.ipynb` notes with cells that run on this machine, and ▶ Run on fenced code
// blocks in Markdown notes. See NotebookEditor / RunnableCodeBlock.
import { notebooksApi } from '@/lib/api/notebooks';
import { notesApi } from '@/lib/api/notes';

export { NotebookEditor, default } from './NotebookEditor';
export { RunnableCodeBlock, useRunnableCodeBlocks, useRunnableCodeExtras } from './RunnableCodeBlock';
export { OutputList, RunBadge } from './CellOutputs';

/**
 * Creates `<folder>/<slug(title)>.ipynb` from the starter template and trusts it right away — the
 * user just made it, so its first run shouldn't ask. Returns the new note path.
 */
export async function createNotebook(root: string, folder: string, title: string): Promise<string> {
  const path = await notesApi.createNotebook(root, folder, title);
  try {
    await notebooksApi.trust(root, path, await notesApi.read(root, path));
  } catch {
    // Not fatal: the notebook will ask for trust on its first run.
  }
  return path;
}
