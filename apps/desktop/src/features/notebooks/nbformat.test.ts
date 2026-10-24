import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { stripAnsi, applyCarriageReturns } from './ansi';
import { cellLanguage, newCell, parseNotebook, serializeNotebook, toNbOutputs, withDevallMeta, withLanguage, type EditCell } from './nbformat';

const example = readFileSync(fileURLToPath(new URL('../../../../../examples/jsonplaceholder/notes/run-the-app.ipynb', import.meta.url)), 'utf8');
const keep = { saveOutputs: false, outputs: () => undefined };

const cell = (source: string, metadata: Record<string, unknown> = {}): EditCell => ({ id: 'x', type: 'code', source, raw: { cell_type: 'code', metadata, source } });

describe('nbformat', () => {
  it('round-trips a Jupyter-formatted notebook byte for byte', () => {
    const { doc, cells } = parseNotebook(example);
    expect(cells.length).toBeGreaterThan(5);
    expect(serializeNotebook(doc, cells, keep)).toBe(example);
  });

  it('keeps unknown fields and adds cell ids to old notebooks', () => {
    const text = JSON.stringify(
      {
        cells: [{ attachments: { 'a.png': {} }, cell_type: 'markdown', metadata: { tags: ['x'] }, source: ['# T\n', 'body'] }],
        metadata: { custom: { keep: true } },
        nbformat: 4,
        nbformat_minor: 4,
        zzz: 1,
      },
      null,
      2,
    );
    const { doc, cells } = parseNotebook(text);
    expect(cells[0].source).toBe('# T\nbody');
    const out = JSON.parse(serializeNotebook(doc, cells, keep));
    expect(out.zzz).toBe(1);
    expect(out.metadata.custom.keep).toBe(true);
    expect(out.cells[0].attachments).toEqual({ 'a.png': {} });
    expect(out.cells[0].metadata.tags).toEqual(['x']);
    expect(typeof out.cells[0].id).toBe('string');
    expect(out.nbformat_minor).toBe(5);
    expect(serializeNotebook(doc, cells, keep).startsWith('{\n  "cells"')).toBe(true);
  });

  it('saves outputs only when asked', () => {
    const text = JSON.stringify({ cells: [{ cell_type: 'code', execution_count: 3, id: 'c1', metadata: {}, outputs: [{ output_type: 'stream', name: 'stdout', text: ['old\n'] }], source: 'ls' }], metadata: {}, nbformat: 4, nbformat_minor: 5 });
    const { doc, cells } = parseNotebook(text);
    const off = JSON.parse(serializeNotebook(doc, cells, keep));
    expect(off.cells[0].outputs).toEqual([]);
    expect(off.cells[0].execution_count).toBeNull();
    const untouched = JSON.parse(serializeNotebook(doc, cells, { saveOutputs: true, outputs: () => undefined }));
    expect(untouched.cells[0].outputs[0].text).toEqual(['old\n']);
    const fresh = JSON.parse(
      serializeNotebook(doc, cells, {
        saveOutputs: true,
        outputs: () => ({ outputs: [{ kind: 'stream', name: 'stdout', text: '\u001b[32mok\u001b[0m\n' }, { kind: 'notice', text: 'not saved' }], executionCount: 9 }),
      }),
    );
    expect(fresh.cells[0].outputs).toEqual([{ output_type: 'stream', name: 'stdout', text: ['ok\n'] }]);
    expect(fresh.cells[0].execution_count).toBe(9);
  });

  it('finds a cell language like devall-core', () => {
    const top = { metadata: { kernelspec: { language: 'python' } } };
    expect(cellLanguage(cell('%%bash\nnpm run dev\n'), top)).toEqual({ language: 'shell', code: 'npm run dev\n', magic: true });
    expect(cellLanguage(cell('console.log(1)', { vscode: { languageId: 'javascript' } }), top).language).toBe('javascript');
    expect(cellLanguage(cell('dir', { devall: { language: 'cmd' }, vscode: { languageId: 'javascript' } }), top).language).toBe('cmd');
    expect(cellLanguage(cell('print(1)'), top).language).toBe('python');
    expect(cellLanguage(cell('%%time\nx'), top)).toEqual({ language: 'python', code: '%%time\nx', magic: false });
    expect(cellLanguage(cell('x'), { metadata: {} }).language).toBeNull();
  });

  it('edits languages and flags', () => {
    const c = withLanguage(cell('%%bash\nls', { vscode: { languageId: 'shellscript' } }), 'powershell');
    expect(c.source).toBe('ls');
    expect(c.raw.metadata).toEqual({ vscode: { languageId: 'powershell' }, devall: { language: 'powershell' } });
    const t = withDevallMeta(c, { templating: true });
    expect((t.raw.metadata as { devall: Record<string, unknown> }).devall.templating).toBe(true);
    const u = withDevallMeta(t, { templating: undefined });
    expect((u.raw.metadata as { devall: Record<string, unknown> }).devall).toEqual({ language: 'powershell' });
    expect(newCell('code', 'python').raw.metadata).toEqual({ devall: { language: 'python' } });
  });

  it('converts outputs and terminal text', () => {
    expect(toNbOutputs([{ kind: 'error', ename: 'E', evalue: '\u001b[1mbad\u001b[0m', traceback: ['\u001b[31mline\u001b[0m'] }], null)).toEqual([{ output_type: 'error', ename: 'E', evalue: 'bad', traceback: ['line'] }]);
    expect(stripAnsi('\u001b]0;title\u0007a\u001b[2Kb')).toBe('ab');
    expect(applyCarriageReturns('10%\r50%\r100%\ndone\r\n')).toBe('100%\ndone\n');
  });
});
