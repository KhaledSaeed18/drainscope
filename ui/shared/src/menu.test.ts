import { describe, expect, it } from 'vitest';

import { describeConsumer, iconName, parseConsumer } from './consumers';
import { buildMenu } from './menu';
import { decodeSummary, type Summary } from './monitor';
import { formatDuration, formatEnergy, formatPercent } from './units';

const appNames = (id: string): string | undefined =>
  id === 'org.mozilla.firefox' ? 'Firefox' : undefined;

describe('decodeSummary', () => {
  it('accepts a well-formed reply', () => {
    const decoded = decodeSummary([true, 1_791_000_000, 18.2, [['app:org.mozilla.firefox', 3024, 2.1, 61]]]);
    expect(decoded).toEqual({
      ok: true,
      value: {
        onBattery: true,
        sinceUnplug: 1_791_000_000,
        batteryPercent: 18.2,
        top: [{ key: 'app:org.mozilla.firefox', joules: 3024, ofBattery: 2.1, ofActive: 61 }],
      },
    });
  });

  it('rejects malformed replies', () => {
    expect(decodeSummary(null).ok).toBe(false);
    expect(decodeSummary([true, 1, 2]).ok).toBe(false);
    expect(decodeSummary(['yes', 1, 2, []]).ok).toBe(false);
    expect(decodeSummary([true, 1, 2, [['app:x', 'lots', 0, 0]]]).ok).toBe(false);
  });
});

describe('consumers', () => {
  it('parses keys like the daemon writes them', () => {
    expect(parseConsumer('app:org.mozilla.firefox')).toEqual({ kind: 'app', name: 'org.mozilla.firefox' });
    expect(parseConsumer('user-unit:dbus-:1.2-x@0.service')).toEqual({ kind: 'user-unit', name: 'dbus-:1.2-x@0.service' });
    expect(parseConsumer('devices')).toEqual({ kind: 'devices', name: '' });
    expect(parseConsumer('martian:x')).toEqual({ kind: 'unknown', name: 'martian:x' });
    expect(parseConsumer('app:')).toEqual({ kind: 'unknown', name: 'app:' });
  });

  it('labels and icons match the CLI', () => {
    expect(describeConsumer(parseConsumer('app:org.mozilla.firefox'), appNames)).toBe('Firefox');
    expect(describeConsumer(parseConsumer('app:org.unknown'), appNames)).toBe('org.unknown');
    expect(describeConsumer(parseConsumer('unit:NetworkManager.service'), appNames)).toBe('System: NetworkManager');
    expect(iconName(parseConsumer('term:pnpm'))).toBe('utilities-terminal-symbolic');
  });
});

describe('formatting', () => {
  it('matches the CLI', () => {
    expect(formatDuration(4320)).toBe('1 h 12 min');
    expect(formatDuration(2 * 86_400 + 3 * 3600)).toBe('2 d 3 h');
    expect(formatDuration(35)).toBe('35 s');
    expect(formatEnergy(3600 * 12.34)).toBe('12.3 Wh');
    expect(formatEnergy(17.3)).toBe('0.005 Wh');
    expect(formatPercent(-0)).toBe('0%');
  });
});

describe('buildMenu', () => {
  const now = 1_791_004_320;
  const summary: Summary = {
    onBattery: true,
    sinceUnplug: now - 4320,
    batteryPercent: 18.2,
    top: [
      { key: 'devices', joules: 16_632, ofBattery: 11, ofActive: 0 },
      { key: 'app:org.mozilla.firefox', joules: 3024, ofBattery: 2, ofActive: 61 },
      { key: 'term:pnpm', joules: 828, ofBattery: 0.5, ofActive: 17 },
    ],
  };

  it('summarizes the current discharge', () => {
    const menu = buildMenu(summary, appNames, now);
    expect(menu.headline).toBe('On battery for 1 h 12 min: 18% used');
    expect(menu.subtitle).toBe('Firefox · 18% used');
    expect(menu.rows.map((r) => [r.label, r.share, r.detail])).toEqual([
      ['Display & devices', '11%', '4.62 Wh'],
      ['Firefox', '2%', '61% of active use · 0.84 Wh'],
      ['Terminal: pnpm', '<1%', '17% of active use · 0.23 Wh'],
    ]);
  });

  it('describes a finished discharge on AC', () => {
    const menu = buildMenu({ ...summary, onBattery: false }, appNames, now);
    expect(menu.headline).toBe('Last discharge, started 1 h 12 min ago: 18% used');
  });

  it('explains when nothing was measured yet', () => {
    const menu = buildMenu({ onBattery: false, sinceUnplug: 0, batteryPercent: 0, top: [] }, appNames, now);
    expect(menu.subtitle).toBe('Unplug to measure');
    expect(menu.rows).toEqual([]);
  });
});
