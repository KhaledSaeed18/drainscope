/** What the quick-settings menu shows, computed from a `GetSummary` reply. */

import { describeConsumer, isAttributable, parseConsumer, type Consumer } from './consumers';
import type { Summary } from './monitor';
import { formatDuration, formatEnergy, formatPercent } from './units';

export interface MenuRow {
  consumer: Consumer;
  label: string;
  /** Share of the battery, e.g. `14%`. */
  share: string;
  /** Secondary text, e.g. `61% of active use · 0.84 Wh`. */
  detail: string;
}

export interface MenuModel {
  /** Toggle subtitle: the most expensive attributable consumer, or a hint. */
  subtitle: string;
  /** Menu header line. */
  headline: string;
  rows: MenuRow[];
}

/** Rows shown in the menu. */
export const MENU_ROWS = 8;

export function buildMenu(
  summary: Summary,
  appName: (id: string) => string | undefined,
  nowSeconds: number,
): MenuModel {
  if (summary.sinceUnplug === 0) {
    return {
      subtitle: summary.onBattery ? 'Measuring…' : 'Unplug to measure',
      headline: summary.onBattery
        ? 'On battery; measuring since drainscope started.'
        : 'No discharge recorded yet. Unplug the charger to start measuring.',
      rows: [],
    };
  }
  const ago = formatDuration(nowSeconds - summary.sinceUnplug);
  const used = formatPercent(summary.batteryPercent);
  const headline = summary.onBattery
    ? `On battery for ${ago}: ${used} used`
    : `Last discharge, started ${ago} ago: ${used} used`;

  const rows = summary.top.slice(0, MENU_ROWS).map((top) => {
    const consumer = parseConsumer(top.key);
    const energy = formatEnergy(top.joules);
    return {
      consumer,
      label: describeConsumer(consumer, appName),
      share: formatPercent(top.ofBattery),
      detail: isAttributable(consumer) ? `${formatPercent(top.ofActive)} of active use · ${energy}` : energy,
    };
  });
  const leader = rows.find((row) => isAttributable(row.consumer));
  return {
    subtitle: leader === undefined ? `${used} used` : `${leader.label} · ${used} used`,
    headline,
    rows,
  };
}
