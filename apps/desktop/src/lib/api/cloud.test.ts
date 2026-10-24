import { describe, expect, it } from 'vitest';
import { intervalLabel, MONITOR_INTERVALS } from './cloud';

describe('monitor intervals', () => {
  it('labels every interval the relay accepts', () => {
    expect(MONITOR_INTERVALS.map(intervalLabel)).toEqual(['every 5 min', 'every 10 min', 'every 15 min', 'every 30 min', 'hourly', 'every 3 h', 'every 6 h', 'every 12 h', 'daily']);
  });
});
