import { describe, expect, it } from 'vitest';

import {
  decodeBatteryHealth,
  decodeCoverage,
  decodeFocus,
  decodeSleepHistory,
  decodeNetwork,
  decodeUsage,
  decodeWakeups,
  focusAppId,
} from './monitor';
import {
  bannerTitle,
  buildHealth,
  buildNetwork,
  buildSleep,
  buildUsage,
  buildWakeups,
  daemonProblem,
  dischargeKey,
  rangeQuery,
} from './views';

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
    expect(decodeSleepHistory([[[100, 3700, 0.4, Number.NaN, 'deep', 'Lid (PNP0C0D:00)']]]).ok).toBe(true);
    expect(decodeSleepHistory([[[100, 3700, 0.4, Number.NaN, 'deep']]]).ok).toBe(false);
    expect(decodeBatteryHealth([[['BAT0', 100, 30.9, 39, 312]]])).toEqual({
      ok: true,
      value: [{ battery: 'BAT0', time: 100, fullWh: 30.9, designWh: 39, cycles: 312 }],
    });
    expect(decodeBatteryHealth([[['BAT0', 100, 30.9, 39]]]).ok).toBe(false);
  });

  it('decode focus replies', () => {
    expect(decodeFocus([[['app:org.mozilla.firefox', 120, 30, 4]]])).toEqual({
      ok: true,
      value: [{ key: 'app:org.mozilla.firefox', foreground: 120, unknown: 30, focusedSeconds: 4 }],
    });
    expect(decodeFocus([[['app:org.mozilla.firefox', 120, 30]]]).ok).toBe(false);
    expect(decodeFocus([]).ok).toBe(false);
  });
});

describe('focusAppId', () => {
  it('strips .desktop and reports nothing for window-backed apps', () => {
    expect(focusAppId('org.mozilla.firefox.desktop')).toBe('org.mozilla.firefox');
    expect(focusAppId('window:42')).toBe('');
    expect(focusAppId(null)).toBe('');
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

  it('splits apps by focus when the daemon knows it', () => {
    const app = (key: string, total: number) => ({ key, kind: 'app', total, cpu: total, gpu: 0, other: 0 });
    const usage = [app('app:org.mozilla.firefox', 9000), app('app:org.gnome.Software', 1800), ...rows.slice(0, 1)];
    const focus = [
      { key: 'app:org.mozilla.firefox', foreground: 7560, unknown: 0, focusedSeconds: 2100 },
      // Mostly before the extension reported focus; the rest in the background.
      { key: 'app:org.gnome.Software', foreground: 0, unknown: 360, focusedSeconds: 0 },
    ];
    const model = buildUsage(usage, 3600, 3600, false, appNames, focus);
    const [firefox, software, devices] = model.entries;
    expect(firefox?.focus).toEqual({
      inUse: '2.10 Wh over 35 min',
      background: '0.40 Wh',
      unknown: undefined,
      mostlyBackground: false,
    });
    expect(software?.focus).toEqual({ inUse: '0.000 Wh over 0 s', background: '0.40 Wh', unknown: '0.10 Wh', mostlyBackground: true });
    expect(devices?.focus).toBeUndefined();
    // No data for the app (older daemon or history before focus tracking).
    expect(buildUsage(usage, 3600, 3600, false, appNames).entries[0]?.focus).toBeUndefined();
    // Only unknown energy, e.g. right after upgrading: nothing to show rather than "0 Wh over 0 s".
    const allUnknown = [{ key: 'app:org.mozilla.firefox', foreground: 0, unknown: 9000, focusedSeconds: 0 }];
    expect(buildUsage(usage, 3600, 3600, false, appNames, allUnknown).entries[0]?.focus).toBeUndefined();
  });

  it('folds small consumers into one entry', () => {
    const many = Array.from({ length: 5 }, (_, i) => ({
      key: `unit:s${String(i)}.service`,
      kind: 'unit',
      total: 50 - i,
      cpu: 50 - i,
      gpu: 0,
      other: 0,
    }));
    const model = buildUsage(many, 60, 60, false, appNames, [], 2);
    expect(model.entries.map((e) => [e.label, e.energy])).toEqual([
      ['System: s0', '0.014 Wh'],
      ['System: s1', '0.014 Wh'],
      ['3 others', '0.039 Wh'],
    ]);
    expect(model.entries[2]?.consumer).toBeUndefined();
    // One extra row isn't worth folding.
    expect(buildUsage(many.slice(0, 3), 60, 60, false, appNames, [], 2).entries).toHaveLength(3);
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
        { start: 1000, end: 1000 + 8 * 3600, whLost: 1.6, percentLost: 4, mode: 'deep', wakeReason: 'Lid (PNP0C0D:00)' },
        { start: 20_000, end: 20_600, whLost: Number.NaN, percentLost: Number.NaN, mode: '', wakeReason: '' },
      ],
      now,
    );
    expect(entries).toEqual([
      { key: '20000', title: 'Slept 10 min, lost ?', subtitle: '3 h 43 min ago · ? Wh · ?%/h' },
      { key: '1000', title: 'Slept 8 h, lost 4%', subtitle: '9 h ago · 1.60 Wh · 0.5%/h · deep · woke: Lid (PNP0C0D:00)' },
    ]);
  });

  it('shows the charge gained on AC, not a negative loss', () => {
    const night = { start: 1000, end: 1000 + 8 * 3600, mode: 'deep', wakeReason: '' };
    const now = 1000 + 9 * 3600;
    expect(buildSleep([{ ...night, whLost: -12.3, percentLost: -31 }], now)).toEqual([
      { key: '1000', title: 'Slept 8 h, charging', subtitle: '9 h ago · +12.30 Wh · deep' },
    ]);
    // Noise around zero is no charge.
    expect(buildSleep([{ ...night, whLost: -0.01, percentLost: -0.02 }], now)).toEqual([
      { key: '1000', title: 'Slept 8 h, lost 0%', subtitle: '9 h ago · 0.00 Wh · 0.0%/h · deep' },
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

describe('dischargeKey', () => {
  it('only the unplug range depends on the discharge', () => {
    expect(dischargeKey('unplug', 4000)).toBe(4000);
    expect(dischargeKey('week', 4000)).toBe(0);
  });
});

describe('bannerTitle', () => {
  const summary = { onBattery: true, sinceUnplug: 4000, batteryPercent: 12, top: [] };
  it('describes the current discharge', () => {
    expect(bannerTitle(summary, 4000 + 3600)).toBe('On battery for 1 h · 12% used');
  });
  it('does not count from the epoch when no unplug was recorded', () => {
    const title = bannerTitle({ ...summary, sinceUnplug: 0 }, 1_800_000_000);
    expect(title).toBe('On battery · measuring since drainscope started');
    expect(title).not.toMatch(/\d+ d\b/);
  });
  it('describes the last discharge when plugged in', () => {
    expect(bannerTitle({ ...summary, onBattery: false }, 9000)).toBe('Plugged in · the last discharge used 12%');
  });
  it('is plain when there was no discharge', () => {
    expect(bannerTitle({ ...summary, onBattery: false, sinceUnplug: 0 }, 9000)).toBe('Plugged in');
  });
});

describe('buildHealth', () => {
  it('shows the latest reading per battery and the change', () => {
    const day = 86_400;
    const entries = buildHealth([
      { battery: 'BAT0', time: 0, fullWh: 31.2, designWh: 39, cycles: 300 },
      { battery: 'BAT1', time: 0, fullWh: 30, designWh: Number.NaN, cycles: 0 },
      { battery: 'BAT0', time: 30 * day, fullWh: 30.9, designWh: 39, cycles: 312 },
    ]);
    expect(entries).toEqual([
      {
        battery: 'BAT0',
        title: '79% of design',
        subtitle: '30.9 of 39.0 Wh · 312 cycles · −0.3 Wh in 30 d',
        fraction: 30.9 / 39,
      },
      { battery: 'BAT1', title: 'Design capacity unknown', subtitle: '30.0 Wh', fraction: undefined },
    ]);
  });
});

describe('buildWakeups', () => {
  it('decodes and lists the top wakers', () => {
    const decoded = decodeWakeups([true, [['app:org.mozilla.firefox', 41.2], ['unit:NetworkManager.service', 2.46]]]);
    expect(decoded.ok).toBe(true);
    if (!decoded.ok) {
      return;
    }
    expect(buildWakeups(decoded.value, 5, appNames)).toEqual([
      {
        key: 'app:org.mozilla.firefox',
        consumer: { kind: 'app', name: 'org.mozilla.firefox' },
        label: 'Firefox',
        rate: '41/s',
        fraction: 1,
      },
      {
        key: 'unit:NetworkManager.service',
        consumer: { kind: 'unit', name: 'NetworkManager.service' },
        label: 'System: NetworkManager',
        rate: '2.5/s',
        fraction: 2.46 / 41.2,
      },
    ]);
    expect(buildWakeups({ available: false, rates: [] }, 5, appNames)).toBeUndefined();
    expect(decodeWakeups([true, [['x']]]).ok).toBe(false);
  });
});

describe('buildNetwork', () => {
  it('decodes and lists the busiest consumers', () => {
    const decoded = decodeNetwork([true, [['app:org.mozilla.firefox', 1_520_000, 41_000], ['unit:dnf.service', 850, 40]]]);
    expect(decoded.ok).toBe(true);
    if (!decoded.ok) {
      return;
    }
    const entries = buildNetwork(decoded.value, 1, appNames);
    expect(entries).toEqual([
      {
        key: 'app:org.mozilla.firefox',
        consumer: { kind: 'app', name: 'org.mozilla.firefox' },
        label: 'Firefox',
        traffic: '↓ 1.5 MB/s · ↑ 41.0 kB/s',
        fraction: 1,
      },
    ]);
    expect(buildNetwork({ available: false, rates: [] }, 5, appNames)).toBeUndefined();
    expect(decodeNetwork([true, [['x', 1]]]).ok).toBe(false);
  });
});

describe('daemonProblem', () => {
  it('says not running only when nothing can answer', () => {
    expect(
      daemonProblem('GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown: The name is not activatable').title,
    ).toBe('drainscope isn’t running');
    expect(
      daemonProblem('GDBus.Error:org.freedesktop.DBus.Error.NameHasNoOwner: Could not activate remote peer').title,
    ).toBe('drainscope isn’t running');
    // Running, but its history query failed.
    expect(daemonProblem('GDBus.Error:org.freedesktop.DBus.Error.Failed: reading usage history failed')).toEqual({
      title: 'drainscope couldn’t read its history',
      action: 'Its log says why:',
      command: 'journalctl --user -u drainscope.service -n 20',
    });
    // Enabled, but exiting at start (e.g. a database from a newer drainscope).
    expect(daemonProblem('GDBus.Error:org.freedesktop.systemd1.UnitFailed: Unit drainscope.service failed').title).toBe(
      'drainscope isn’t answering',
    );
  });
});
