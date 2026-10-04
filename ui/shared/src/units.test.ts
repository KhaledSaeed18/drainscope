import { describe, expect, it } from 'vitest';

import { batteryPercent, formatPercent, joulesToWattHours } from './units';

describe('joulesToWattHours', () => {
  it('converts 3600 J to 1 Wh', () => {
    expect(joulesToWattHours(3600)).toBe(1);
  });
});

describe('batteryPercent', () => {
  it('expresses energy as a share of capacity', () => {
    expect(batteryPercent(3600 * 4.2, 42)).toBeCloseTo(10);
  });

  it('is undefined without a usable capacity', () => {
    expect(batteryPercent(100, 0)).toBeUndefined();
    expect(batteryPercent(100, Number.NaN)).toBeUndefined();
  });
});

describe('formatPercent', () => {
  it('rounds to whole percent', () => {
    expect(formatPercent(13.6)).toBe('14%');
  });

  it('shows tiny non-zero shares as <1%', () => {
    expect(formatPercent(0.3)).toBe('<1%');
    expect(formatPercent(0)).toBe('0%');
  });
});
