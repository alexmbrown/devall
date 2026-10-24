/**
 * Whether a natural-language request explicitly asks to change data (insert, update, delete…).
 * NL → SQL only lets the model write a non-SELECT statement when this is true *and* the session
 * is read-write; the database's EXPLAIN still checks it and nothing is executed.
 */
export function explicitWrite(question: string): boolean {
  const q = ` ${question.toLowerCase()} `;
  return /\b(insert|update|delete|remove|drop|truncate|alter|create|add (a |an |new )?(row|record|column|user|entry)|set .+ to|change .+ to|rename|upsert|merge into)\b/.test(q);
}

/** Splits a MongoDB find suggestion back into the workspace fields (empty strings stay empty). */
export function mongoFields(s: { filter: string; projection: string; sort: string; limit: number }): { filter: string; projection: string; sort: string; limit?: number } {
  return { filter: s.filter.trim() || '{}', projection: s.projection.trim(), sort: s.sort.trim(), limit: s.limit > 0 ? s.limit : undefined };
}
