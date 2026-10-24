/**
 * Saving and picking files from the webview. In the desktop app files are saved through the
 * native save dialog (text via `write_text_file`, bytes via `wiremock_write_file` when that command
 * is registered); in a plain browser (or as a last resort) a blob download is used.
 */
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { api, errorMessage, isTauri } from '@/lib/ipc';
import { toast } from '@/components/ui/toast';
import { writeBytes } from './transport';

function anchorDownload(name: string, blob: Blob) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

function filtersFor(name: string) {
  const ext = /\.([a-z0-9]+)$/i.exec(name)?.[1];
  return ext ? [{ name: ext.toUpperCase(), extensions: [ext] }] : undefined;
}

async function pickTarget(name: string): Promise<string | null> {
  return saveDialog({ defaultPath: name, filters: filtersFor(name) });
}

/** Saves text through the native save dialog. Resolves to the path, or null when cancelled. */
export async function saveText(name: string, text: string, type = 'text/plain'): Promise<string | null> {
  if (!isTauri) {
    anchorDownload(name, new Blob([text], { type }));
    return name;
  }
  try {
    const path = await pickTarget(name);
    if (!path) return null;
    await api.writeTextFile(path, text);
    toast.success('Saved', path);
    return path;
  } catch (e) {
    toast.error('Could not save the file', errorMessage(e));
    return null;
  }
}

/** Saves bytes through the native save dialog (blob download if the byte-writing command is missing). */
export async function saveBytes(name: string, bytes: Uint8Array, type = 'application/octet-stream'): Promise<string | null> {
  if (!isTauri) {
    anchorDownload(name, new Blob([bytes.slice().buffer], { type }));
    return name;
  }
  try {
    const path = await pickTarget(name);
    if (!path) return null;
    if (await writeBytes(path, bytes)) {
      toast.success('Saved', path);
      return path;
    }
    anchorDownload(name, new Blob([bytes.slice().buffer], { type }));
    toast.info('Downloading via the webview', 'Register wiremock_write_file to save binary files with the native dialog.');
    return name;
  } catch (e) {
    toast.error('Could not save the file', errorMessage(e));
    return null;
  }
}

export async function downloadBlob(name: string, blob: Blob) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  if (blob.type.startsWith('text/') || blob.type.includes('json')) return saveText(name, new TextDecoder().decode(bytes), blob.type);
  return saveBytes(name, bytes, blob.type || undefined);
}

export function downloadJson(name: string, data: unknown) {
  return saveText(name, JSON.stringify(data, null, 2), 'application/json');
}

export function downloadText(name: string, text: string, type = 'text/plain') {
  return saveText(name, text, type);
}

export function pickFiles(accept?: string, multiple = true): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = multiple;
    if (accept) input.accept = accept;
    input.onchange = () => resolve([...(input.files ?? [])]);
    input.oncancel = () => resolve([]);
    input.click();
  });
}

export function safeFileName(s: string): string {
  return s.replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '').slice(0, 80) || 'stub';
}
