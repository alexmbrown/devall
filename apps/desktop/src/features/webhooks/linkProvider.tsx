import { Webhook } from 'lucide-react';
import { webhooksApi } from '@/lib/api/webhooks';
import { linkMessage, type LinkProvider } from '@/lib/links/registry';
import { listSource, NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';

const inboxes = listSource({ folders: ['webhooks/'], load: (root) => webhooksApi.list(root) });

/** `devall:webhook/<inbox id>` (the file stem under webhooks/). */
export const webhookLinks: LinkProvider = {
  kinds: ['webhook'],
  items: () => (inboxes.get() ?? []).map((i) => ({ link: formatLink('webhook', i.id), kind: 'webhook', title: i.file?.name ?? i.id, detail: `webhooks/${i.id}.webhook.yaml`, icon: <Webhook /> })),
  resolve(_kind, id) {
    if (!projectRoot()) return NO_PROJECT;
    const list = inboxes.get();
    if (!list) return undefined;
    const i = list.find((x) => x.id === id);
    return i ? { title: i.file?.name ?? i.id, detail: `webhooks/${i.id}.webhook.yaml`, note: i.error ?? undefined } : null;
  },
  open(_kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    go('webhooks', id);
  },
  icon: () => <Webhook />,
};
