import type { ApprovalRequest, McpWriteMode } from '@/lib/api/ai';

/** Whole seconds until the backend denies `req` on its own (never negative). */
export function secondsLeft(req: Pick<ApprovalRequest, 'createdAt' | 'timeoutSecs'>, now: number): number {
  return Math.max(0, Math.ceil((req.createdAt + req.timeoutSecs * 1000 - now) / 1000));
}

/** Flags for the MCP server: stdio snippets only know `--allow-writes` (no UI to ask). */
export function writeFlags(mode: McpWriteMode): { allowWrites: boolean; confirmWrites: boolean } {
  return { allowWrites: mode === 'allow', confirmWrites: mode === 'confirm' };
}
