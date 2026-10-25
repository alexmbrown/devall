import { useCallback } from 'react';
import clsx from 'clsx';
import { Copy, Link2 } from 'lucide-react';
import type { MenuItemDef } from '@/components/ui/Menu';
import { copyText, isMac } from '@/components/ui/misc';
import { linkTooltip, openLink, useLinkInfo } from '@/lib/links/registry';
import { markdownLink } from '@/lib/links/syntax';
import { useNav } from '@/store/nav';
import { kindIcon } from './kindIcons';
import './links.css';

export const OPEN_HINT = `Click to open · ${isMac ? '⌘' : 'Ctrl'}+click or middle-click: open to the side`;

/**
 * A `devall:` link as an inline chip: the kind's icon (method badge, to-do check circle,
 * environment colour…), the item's live title, muted and struck through when it's gone, a short
 * flag when it can't be resolved here (personal to-do from another machine, no project…).
 * Presentational: the parent handles clicks (`useFollowLink`).
 */
export function LinkChip({ href, label, className }: { href: string; label?: string; className?: string }) {
  const info = useLinkInfo(href, label);
  return (
    <span className={clsx('dl-chip', `dl-${info.state}`, info.done && 'dl-done', className)} title={`${linkTooltip(info)}\n${OPEN_HINT}`} data-kind={info.kind ?? undefined}>
      <span className="dl-icon" aria-hidden>
        {info.icon ?? kindIcon(info.kind)}
      </span>
      <span className="dl-title">{info.title}</span>
      {info.badge}
      {info.state === 'missing' && <span className="dl-flag">not found</span>}
      {(info.state === 'unavailable' || info.state === 'invalid') && <span className="dl-flag">{info.note}</span>}
    </span>
  );
}

/** Whether a click/auxclick asks for the neighbouring group (Ctrl/Cmd+click, middle click). */
export const wantsSide = (e: { ctrlKey: boolean; metaKey: boolean; button?: number }) => e.ctrlKey || e.metaKey || e.button === 1;

/** Follows `devall:` links from the clicked view (current group; to the side with Ctrl/Cmd or the middle button). */
export function useFollowLink() {
  const go = useNav((s) => s.go);
  return useCallback((href: string, e?: { ctrlKey: boolean; metaKey: boolean; button?: number }) => void openLink(href, { go, toSide: !!e && wantsSide(e) }), [go]);
}

/** "Copy link" / "Copy as Markdown link" menu items for an item's `devall:` link. */
export function copyLinkItems(link: string, title: string): MenuItemDef[] {
  return [
    { label: 'Copy link', icon: <Link2 />, onSelect: () => void copyText(link, 'Link copied') },
    { label: 'Copy as Markdown link', icon: <Copy />, onSelect: () => void copyText(markdownLink(title, link), 'Markdown link copied') },
  ];
}
