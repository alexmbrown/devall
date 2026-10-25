import { formatLink, pageLink } from './syntax';

/**
 * The `devall:` link for a workbench route (a view's `section` + `param`): the item the page shows
 * when the route names one (a request, note, environment, load test, connection, saved query,
 * utility, repository…), else the generic `devall:page/<section>[/<param>]`. "Copy link to this
 * page" uses it. One-off parts of params (`|nonce`, a utility's `|value`) are dropped.
 */
export function linkForRoute(section: string, param?: string): string {
  const p = param?.split('|')[0] || undefined;
  if (!p) return pageLink(section);
  switch (section) {
    case 'http':
      if (p === '_folder.yaml') return pageLink('http');
      return p.endsWith('/_folder.yaml') ? formatLink('folder', p.slice(0, -'/_folder.yaml'.length)) : formatLink('request', p);
    case 'notes':
      return formatLink('note', p);
    case 'environments':
      return p === '__project' ? formatLink('variables') : p.startsWith('__') ? pageLink(section, p) : formatLink('env', p);
    case 'load':
      return formatLink('load', p);
    case 'fakedata':
      return formatLink('fakedata', p);
    case 'data':
      return p.startsWith('query:') ? formatLink('query', p.slice('query:'.length)) : formatLink('connection', p);
    case 'webhooks':
      return formatLink('webhook', p);
    case 'utilities':
      return formatLink('utility', p);
    case 'git':
      return formatLink('repo', p);
    default:
      return pageLink(section, p);
  }
}
