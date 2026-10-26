import { describe, expect, it } from 'vitest';
import { noteImagePath } from './images';

describe('noteImagePath', () => {
  it('resolves against the note’s folder', () => {
    expect(noteImagePath('img/a.png', 'design/auth.md')).toBe('design/img/a.png');
    expect(noteImagePath('./a%20b.PNG', 'design/auth.md')).toBe('design/a b.PNG');
    expect(noteImagePath('../shared/b.svg?x=1', 'design/auth.md')).toBe('shared/b.svg');
  });
  it('takes project-root paths into notes/', () => {
    expect(noteImagePath('notes/img/c.gif', 'a/b.md')).toBe('img/c.gif');
    expect(noteImagePath('/notes/img/c.gif', 'a/b.md')).toBe('img/c.gif');
  });
  it('rejects URLs, other folders, climbing out and non-images', () => {
    expect(noteImagePath('https://x/y.png', 'a.md')).toBeNull();
    expect(noteImagePath('data:image/png;base64,AA', 'a.md')).toBeNull();
    expect(noteImagePath('/http/x.png', 'a.md')).toBeNull();
    expect(noteImagePath('../../x.png', 'a/b.md')).toBeNull();
    expect(noteImagePath('doc.pdf', 'a.md')).toBeNull();
  });
});
