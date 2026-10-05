/** View models for the desktop app's history and sleep pages. */

import { describeConsumer, parseConsumer, type Consumer } from './consumers';
import type { HealthReading, SleepSession, UsageRow } from './monitor';
import { formatDuration, formatEnergy, formatPercent, formatWatts } from './units';

export interface EnergyPart {
  label: string;
  energy: string;
  /** Fraction of the entry's total, 0–1. */
  fraction: number;
  share: string;
}

export interface UsageEntry {
  /** Storage key, or the kind when grouped by kind. */
  key: string;
  /** `undefined` when grouped by kind. */
  consumer: Consumer | undefined;
  label: string;
  energy: string;
  average: string;
  /** Fraction of the listed total, 0–1, for bars. */
  fraction: number;
  share: string;
  /**
   * Where the energy was measured: the processor package and DRAM, the integrated GPU, or
   * outside RAPL (display, chipset, devices). Empty parts are left out.
   */
  parts: EnergyPart[];
}

export interface UsageModel {
  entries: UsageEntry[];
  /** e.g. `Total 1.82 Wh over 30 min (average 3.65 W)`. */
  footer: string;
}

const KIND_LABELS: Readonly<Record<string, string>> = {
  app: 'Apps',
  term: 'Terminals',
  session: 'Login sessions',
  shell: 'GNOME Shell',
  unit: 'System services',
  'user-unit': 'Your services',
  container: 'Containers',
  'other-users': 'Other users',
  root: 'Root',
  kernel: 'Kernel',
  exited: 'Exited processes',
  idle: 'Idle',
  platform: 'Chipset & platform',
  devices: 'Display & devices',
  drainscope: 'drainscope',
};

export function kindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

/**
 * Rows from `GetUsage` over a span of `spanSeconds`, of which `measuredSeconds` were measured
 * (`GetCoverage`); averages are over measured time.
 */
/** Rows beyond this many (largest first) are folded into one "N others" entry. */
export const USAGE_LIMIT = 15;

/** Keeps the `limit` largest rows (rows come largest first) and sums the rest into one. */
function fold(rows: readonly UsageRow[], limit: number): { kept: readonly UsageRow[]; rest: UsageRow | undefined; count: number } {
  if (rows.length <= limit + 1) {
    return { kept: rows, rest: undefined, count: 0 };
  }
  const tail = rows.slice(limit);
  const rest = tail.reduce(
    (sum, row) => ({
      ...sum,
      total: sum.total + row.total,
      cpu: sum.cpu + row.cpu,
      gpu: sum.gpu + row.gpu,
      other: sum.other + row.other,
    }),
    { key: 'others', kind: 'others', total: 0, cpu: 0, gpu: 0, other: 0 },
  );
  return { kept: rows.slice(0, limit), rest, count: tail.length };
}

export function buildUsage(
  rows: readonly UsageRow[],
  spanSeconds: number,
  measuredSeconds: number,
  byKind: boolean,
  appName: (id: string) => string | undefined,
  limit = USAGE_LIMIT,
): UsageModel {
  const total = rows.reduce((sum, row) => sum + row.total, 0);
  const seconds = Math.max(measuredSeconds, 1);
  const { kept, rest, count } = fold(rows, limit);
  const listed = rest === undefined ? kept : [...kept, rest];
  const entries = listed.map((row) => {
    const folded = row === rest;
    const consumer = byKind || folded ? undefined : parseConsumer(row.key);
    const fraction = total > 0 ? row.total / total : 0;
    const parts = [
      { label: 'Processor & memory', joules: row.cpu },
      { label: 'Graphics', joules: row.gpu },
      { label: 'Display & devices', joules: row.other },
    ]
      .filter((part) => part.joules > 0)
      .map((part) => {
        const partFraction = row.total > 0 ? part.joules / row.total : 0;
        return {
          label: part.label,
          energy: formatEnergy(part.joules),
          fraction: partFraction,
          share: formatPercent(partFraction * 100),
        };
      });
    return {
      key: row.key,
      consumer,
      label: folded
        ? `${String(count)} others`
        : consumer === undefined
          ? kindLabel(row.key)
          : describeConsumer(consumer, appName),
      energy: formatEnergy(row.total),
      average: formatWatts(row.total / seconds),
      fraction,
      share: formatPercent(fraction * 100),
      parts,
    };
  });
  const average = formatWatts(total / seconds);
  const footer =
    measuredSeconds >= spanSeconds * 0.95
      ? `Total ${formatEnergy(total)} over ${formatDuration(spanSeconds)} (average ${average})`
      : `Total ${formatEnergy(total)} in ${formatDuration(measuredSeconds)} measured of the last ${formatDuration(spanSeconds)} (average ${average} while measured)`;
  return { entries, footer };
}

export interface SleepEntry {
  title: string;
  subtitle: string;
}

/** One line per session, newest first. */
export function buildSleep(sessions: readonly SleepSession[], nowSeconds: number): SleepEntry[] {
  return [...sessions]
    .sort((a, b) => b.start - a.start)
    .map((session) => {
      const slept = session.end - session.start;
      const hours = slept / 3600;
      const lost = Number.isFinite(session.percentLost) ? formatPercent(session.percentLost) : '?';
      const energy = Number.isFinite(session.whLost) ? `${session.whLost.toFixed(2)} Wh` : '? Wh';
      const rate =
        hours > 0 && Number.isFinite(session.percentLost)
          ? `${(session.percentLost / hours).toFixed(1)}%/h`
          : '?%/h';
      const mode = session.mode === '' ? '' : ` · ${session.mode}`;
      const woke = session.wakeReason === '' ? '' : ` · woke: ${session.wakeReason}`;
      return {
        title: `Slept ${formatDuration(slept)}, lost ${lost}`,
        subtitle: `${formatDuration(nowSeconds - session.start)} ago · ${energy} · ${rate}${mode}${woke}`,
      };
    });
}

export type Range = 'unplug' | 'hour' | 'day' | 'week';

export const RANGES: readonly { id: Range; label: string }[] = [
  { id: 'unplug', label: 'Since unplugged' },
  { id: 'hour', label: 'Last hour' },
  { id: 'day', label: 'Last 24 hours' },
  { id: 'week', label: 'Last 7 days' },
];

const RANGE_SECONDS: Readonly<Record<Exclude<Range, 'unplug'>, number>> = {
  hour: 3600,
  day: 86_400,
  week: 7 * 86_400,
};

/**
 * `[since, until]` in Unix seconds and the power-source filter for `GetUsage`/`GetCoverage`,
 * or `undefined` for "since unplugged" when the charger was never unplugged.
 */
export function rangeQuery(
  range: Range,
  nowSeconds: number,
  sinceUnplug: number,
): { since: number; until: number; source: string } | undefined {
  if (range === 'unplug') {
    return sinceUnplug > 0 ? { since: sinceUnplug, until: nowSeconds, source: 'battery' } : undefined;
  }
  return { since: nowSeconds - RANGE_SECONDS[range], until: nowSeconds, source: 'any' };
}

export interface HealthEntry {
  battery: string;
  /** e.g. `79% of design`, or `30.9 Wh` when the design capacity is unknown. */
  title: string;
  /** e.g. `30.9 of 39.0 Wh · 312 cycles · −0.3 Wh in 30 d`. */
  subtitle: string;
  /** Full charge over design, 0–1, for a bar; undefined when unknown. */
  fraction: number | undefined;
}

/** The latest reading per battery, with the change since its oldest (readings oldest first). */
export function buildHealth(readings: readonly HealthReading[]): HealthEntry[] {
  const first = new Map<string, HealthReading>();
  const last = new Map<string, HealthReading>();
  for (const reading of readings) {
    if (!first.has(reading.battery)) {
      first.set(reading.battery, reading);
    }
    last.set(reading.battery, reading);
  }
  return [...last.values()].map((latest) => {
    const oldest = first.get(latest.battery) ?? latest;
    const known = Number.isFinite(latest.designWh) && latest.designWh > 0;
    const fraction = known ? latest.fullWh / latest.designWh : undefined;
    const parts = [
      known ? `${latest.fullWh.toFixed(1)} of ${latest.designWh.toFixed(1)} Wh` : `${latest.fullWh.toFixed(1)} Wh`,
    ];
    if (latest.cycles > 0) {
      parts.push(`${String(latest.cycles)} cycles`);
    }
    const span = latest.time - oldest.time;
    if (span > 0) {
      const change = latest.fullWh - oldest.fullWh;
      const sign = change < 0 ? '−' : '+';
      parts.push(`${sign}${Math.abs(change).toFixed(1)} Wh in ${formatDuration(span)}`);
    }
    return {
      battery: latest.battery,
      title: fraction === undefined ? 'Design capacity unknown' : `${formatPercent(fraction * 100)} of design`,
      subtitle: parts.join(' · '),
      fraction,
    };
  });
}
