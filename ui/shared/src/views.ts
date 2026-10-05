/** View models for the desktop app's history and sleep pages. */

import { describeConsumer, parseConsumer, type Consumer } from './consumers';
import type { SleepSession, UsageRow } from './monitor';
import { formatDuration, formatEnergy, formatPercent, formatWatts } from './units';

export interface UsageEntry {
  /** `undefined` when grouped by kind. */
  consumer: Consumer | undefined;
  label: string;
  energy: string;
  average: string;
  /** Fraction of the listed total, 0–1, for bars. */
  fraction: number;
  share: string;
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
export function buildUsage(
  rows: readonly UsageRow[],
  spanSeconds: number,
  measuredSeconds: number,
  byKind: boolean,
  appName: (id: string) => string | undefined,
): UsageModel {
  const total = rows.reduce((sum, row) => sum + row.total, 0);
  const seconds = Math.max(measuredSeconds, 1);
  const entries = rows.map((row) => {
    const consumer = byKind ? undefined : parseConsumer(row.key);
    const fraction = total > 0 ? row.total / total : 0;
    return {
      consumer,
      label: consumer === undefined ? kindLabel(row.key) : describeConsumer(consumer, appName),
      energy: formatEnergy(row.total),
      average: formatWatts(row.total / seconds),
      fraction,
      share: formatPercent(fraction * 100),
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
      return {
        title: `Slept ${formatDuration(slept)}, lost ${lost}`,
        subtitle: `${formatDuration(nowSeconds - session.start)} ago · ${energy} · ${rate}${mode}`,
      };
    });
}
