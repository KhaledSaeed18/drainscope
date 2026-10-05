import { describe, expect, it } from 'vitest';

import { decodeCoverage, decodeSleepSessions, decodeUsage } from './monitor';
import { buildSleep, buildUsage, rangeQuery } from './views';

const appNames = (id: string): string | undefined => (id === 'org.mozilla.firefox' ? 'Firefox' : undefined);

describe('decoders', () => {
  it('decode usage, coverage and sleep replies', () => {
    expect(decodeUsage([[['app:org.mozilla.firefox', 'app', 10, 8, 2, 0]]])).toEqual({
      ok: true,
      value: [{ key: 'app:org.mozilla.firefox', kind: 'app', total: 10, cpu: 8, gpu: 2, other: 0 }],
    });
    expect(decodeUsage([[['x', 'y', 'z', 0, 0, 0]]]).ok).toBe(false);
    expect(decodeCoverage([900])).toEqual({ ok: true, value: 900 });
    expect(decodeCoverage(900).ok).toBe(false);
    expect(decodeSleepSessions([[[100, 3700, 0.4, Number.NaN, 'deep']]]).ok).toBe(true);
    expect(decodeSleepSessions([[[100, 3700, 0.4]]]).ok).toBe(false);
  });
});

describe('buildUsage', () => {
  const rows = [
    { key: 'devices', kind: 'devices', total: 2700, cpu: 0, gpu: 0, other: 2700 },
    { key: 'app:org.mozilla.firefox', kind: 'app', total: 900, cpu: 800, gpu: 100, other: 0 },
  ];

  it('labels, shares and averages over measured time', () => {
    const model = buildUsage(rows, 3600, 1800, false, appNames);
    expect(model.entries.map((e) => [e.label, e.energy, e.average, e.share])).toEqual([
      ['Display & devices', '0.75 Wh', '1.50 W', '75%'],
      ['Firefox', '0.25 Wh', '0.50 W', '25%'],
    ]);
    expect(model.footer).toBe('Total 1.00 Wh in 30 min measured of the last 1 h (average 2.00 W while measured)');
  });

  it('splits each entry by where the energy was measured', () => {
    const model = buildUsage(rows, 3600, 3600, false, appNames);
    expect(model.entries[1]?.parts).toEqual([
      { label: 'Processor & memory', energy: '0.22 Wh', fraction: 800 / 900, share: '89%' },
      { label: 'Graphics', energy: '0.028 Wh', fraction: 100 / 900, share: '11%' },
    ]);
    expect(model.entries[0]?.parts.map((p) => p.label)).toEqual(['Display & devices']);
  });

  it('labels kinds when grouped by kind', () => {
    const byKind = [{ key: 'user-unit', kind: 'user-unit', total: 10, cpu: 10, gpu: 0, other: 0 }];
    const model = buildUsage(byKind, 60, 60, true, appNames);
    expect(model.entries[0]?.label).toBe('Your services');
    expect(model.footer).toBe('Total 0.003 Wh over 1 min (average 0.17 W)');
  });
});

describe('buildSleep', () => {
  it('describes sessions newest first', () => {
    const now = 1000 + 9 * 3600;
    const entries = buildSleep(
      [
        { start: 1000, end: 1000 + 8 * 3600, whLost: 1.6, percentLost: 4, mode: 'deep' },
        { start: 20_000, end: 20_600, whLost: Number.NaN, percentLost: Number.NaN, mode: '' },
      ],
      now,
    );
    expect(entries).toEqual([
      { title: 'Slept 10 min, lost ?', subtitle: '3 h 43 min ago · ? Wh · ?%/h' },
      { title: 'Slept 8 h, lost 4%', subtitle: '9 h ago · 1.60 Wh · 0.5%/h · deep' },
    ]);
  });
});

describe('rangeQuery', () => {
  it('maps ranges to queries', () => {
    expect(rangeQuery('unplug', 5000, 4000)).toEqual({ since: 4000, until: 5000, source: 'battery' });
    expect(rangeQuery('unplug', 5000, 0)).toBeUndefined();
    expect(rangeQuery('hour', 5000, 4000)).toEqual({ since: 1400, until: 5000, source: 'any' });
  });
});
