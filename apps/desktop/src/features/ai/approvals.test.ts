import { describe, expect, it } from 'vitest';
import { secondsLeft, writeFlags } from './approvals';

describe('MCP approvals', () => {
  it('counts down to the automatic denial', () => {
    const req = { createdAt: 10_000, timeoutSecs: 120 };
    expect(secondsLeft(req, 10_000)).toBe(120);
    expect(secondsLeft(req, 10_000 + 119_100)).toBe(1);
    expect(secondsLeft(req, 10_000 + 500_000)).toBe(0);
  });

  it('maps the write mode to server flags', () => {
    expect(writeFlags('readOnly')).toEqual({ allowWrites: false, confirmWrites: false });
    expect(writeFlags('confirm')).toEqual({ allowWrites: false, confirmWrites: true });
    expect(writeFlags('allow')).toEqual({ allowWrites: true, confirmWrites: false });
  });
});
