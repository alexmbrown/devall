import { useEffect, useState } from 'react';
import { todosApi, type Attachment } from '@/lib/api/todos';

/** Image types the webview renders (others show as files). */
const PREVIEWABLE = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/bmp', 'image/svg+xml', 'image/x-icon', 'image/avif'];
const PREVIEW_MAX = 15 * 1024 * 1024;

/** An image attachment the panel previews (and a board card can use as its cover). */
export const previewable = (a: Attachment) => PREVIEWABLE.includes(a.mime) && a.size <= PREVIEW_MAX;

/** Blob URLs of loaded images, newest last (a small cache shared by thumbnails, previews and card covers). */
const urls = new Map<string, Promise<string>>();

export function attachmentUrl(taskId: string, id: string, mime: string): Promise<string> {
  const key = `${taskId}/${id}`;
  let p = urls.get(key);
  if (!p) {
    p = todosApi.attachmentBytes(taskId, id).then((buf) => URL.createObjectURL(new Blob([buf], { type: mime })));
    p.catch(() => urls.delete(key));
    urls.set(key, p);
    if (urls.size > 60) {
      const [oldKey, old] = urls.entries().next().value!;
      urls.delete(oldKey);
      void old.then((u) => URL.revokeObjectURL(u)).catch(() => {});
    }
  }
  return p;
}

export function useAttachmentUrl(taskId: string, a: Attachment): { url: string | null; failed: boolean } {
  const [state, setState] = useState<{ key: string; url: string | null; failed: boolean }>({ key: '', url: null, failed: false });
  const { id, mime } = a;
  const key = `${taskId}/${id}`;
  useEffect(() => {
    let live = true;
    attachmentUrl(taskId, id, mime)
      .then((url) => live && setState({ key, url, failed: false }))
      .catch(() => live && setState({ key, url: null, failed: true }));
    return () => {
      live = false;
    };
  }, [taskId, id, mime, key]);
  return state.key === key ? state : { url: null, failed: false };
}
