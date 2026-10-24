/** Lines of test text used as AI samples are capped so prompts stay small. */
const MAX_LINES = 20;
const MAX_LINE = 200;

/**
 * Samples for "regex from description": the saved should-match cases plus the lines of the test
 * text are positives, the should-not-match cases negatives (a test-text line that is also a
 * negative case is left out of the positives).
 */
export function aiSamples(testText: string, shouldMatch: string[], shouldNotMatch: string[]): { positive: string[]; negative: string[] } {
  const negative = [...new Set(shouldNotMatch)];
  const lines = testText
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l && l.length <= MAX_LINE && !negative.includes(l))
    .slice(0, MAX_LINES);
  return { positive: [...new Set([...shouldMatch, ...lines])], negative };
}
