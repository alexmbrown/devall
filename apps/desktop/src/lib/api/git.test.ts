import { describe, expect, it } from 'vitest';
import { parseRepoLink } from './git';

describe('repo-qualified paths', () => {
  it('parses repo:<name>/<path>', () => {
    expect(parseRepoLink('repo:api/openapi/spec.yaml')).toEqual({ repo: 'api', path: 'openapi/spec.yaml' });
    expect(parseRepoLink('repo:web')).toEqual({ repo: 'web', path: '' });
    expect(parseRepoLink(' repo:web\\src\\app.ts ')).toEqual({ repo: 'web', path: 'src/app.ts' });
    expect(parseRepoLink('repo:')).toBeNull();
    expect(parseRepoLink('http/users.req.yaml')).toBeNull();
    expect(parseRepoLink('C:\\repo:x')).toBeNull();
  });
});
