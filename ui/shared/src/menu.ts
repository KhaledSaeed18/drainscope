/** What the quick-settings menu shows, computed from a `GetSummary` reply. */

import { describeConsumer, isAttributable, parseConsumer, type Consumer } from './consumers';
import type { Summary } from './monitor';
import { formatDuration, formatEnergy, formatPercent } from './units';
import { daemonProblem } from './views';

export interface MenuRow {
  consumer: Consumer;
  label: string;
  /** Share of the battery, e.g. `14%`. */
  share: string;
  /** Energy, e.g. `0.84 Wh`. */
  energy: string;
}

export interface MenuModel {
  /** Toggle subtitle: the most expensive attributable consumer, or a hint. */
  subtitle: string;
  /** Menu header line. */
  headline: string;
  rows: MenuRow[];
}

/** Rows shown in the menu: more don't fit under the quick-settings grid on a 768 px screen. */
export const MENU_ROWS = 5;

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
    return {
      consumer,
      label: describeConsumer(consumer, appName),
      share: formatPercent(top.ofBattery),
      energy: formatEnergy(top.joules),
    };
  });
  const leader = rows.find((row) => isAttributable(row.consumer));
  return {
    // Most important first: a long app name is what gets cut off.
    subtitle: leader === undefined ? `${used} used` : `${used} used · ${leader.label}`,
    headline,
    rows,
  };
}

/** What the Quick Settings tile shows when the daemon can't answer (`reason`: the error). */
export interface MenuProblem {
  /** Short, for the tile's subtitle. */
  subtitle: string;
  /** For the menu header, e.g. `Start it with: systemctl --user enable --now drainscope.service`. */
  headline: string;
}

export function menuProblem(reason: string): MenuProblem {
  const problem = daemonProblem(reason);
  if (problem.title === daemonProblem('org.freedesktop.DBus.Error.ServiceUnknown').title) {
    return { subtitle: 'Not running', headline: `${problem.action}: ${problem.command}` };
  }
  const subtitle =
    problem.title === daemonProblem('org.freedesktop.DBus.Error.Failed').title ? 'Can’t read history' : 'Not answering';
  return { subtitle, headline: `${problem.action} ${problem.command}` };
}
