import { describe, expect, it } from 'vitest';
import { holdRoute, isHeld, releaseRoute } from './hold';

describe('held routes', () => {
  it('holds exactly the routes an outside link opened, for a minute', () => {
    holdRoute('data', 'query:top.sql|1', 1_000);
    expect(isHeld('data', 'query:top.sql|1', 2_000)).toBe(true);
    expect(isHeld('data', 'query:top.sql|2', 2_000)).toBe(false);
    expect(isHeld('data', undefined, 2_000)).toBe(true);
    expect(isHeld('mocks', undefined, 2_000)).toBe(false);
    expect(isHeld('data', 'query:top.sql|1', 61_001)).toBe(false);
  });

  it('releases a route or a whole section', () => {
    const now = Date.now();
    holdRoute('utilities', 'jwt.decode', now);
    holdRoute('utilities', 'recipe.run', now);
    releaseRoute('utilities', 'jwt.decode');
    expect(isHeld('utilities', 'jwt.decode', now)).toBe(false);
    expect(isHeld('utilities', 'recipe.run', now)).toBe(true);
    releaseRoute('utilities');
    expect(isHeld('utilities', undefined, now)).toBe(false);
  });
});
