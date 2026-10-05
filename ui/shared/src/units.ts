/** Energy units shared by the Shell extension and the app. The daemon reports joules. */

const JOULES_PER_WATT_HOUR = 3600;

export function joulesToWattHours(joules: number): number {
  return joules / JOULES_PER_WATT_HOUR;
}

/** Percent of battery capacity that `joules` represents, or undefined without a capacity. */
export function batteryPercent(joules: number, capacityWattHours: number): number | undefined {
  if (!(capacityWattHours > 0)) {
    return undefined;
  }
  return (joulesToWattHours(joules) / capacityWattHours) * 100;
}

/** Whole percent; small non-zero shares read as "<1%" instead of a misleading "0%". */
export function formatPercent(percent: number): string {
  if (percent > 0 && percent < 1) {
    return '<1%';
  }
  if (Math.abs(percent) < 0.5) {
    // Also avoids "-0%".
    return '0%';
  }
  return `${Math.round(percent).toString()}%`;
}

/** `35 s`, `4 min`, `1 h 12 min`, `2 d 3 h` (matches the CLI). */
export function formatDuration(seconds: number): string {
  const secs = Math.max(0, Math.floor(seconds));
  const days = Math.floor(secs / 86_400);
  const hours = Math.floor(secs / 3600) % 24;
  const minutes = Math.floor(secs / 60) % 60;
  if (days > 0) {
    return hours > 0 ? `${days.toString()} d ${hours.toString()} h` : `${days.toString()} d`;
  }
  if (hours > 0) {
    return minutes > 0 ? `${hours.toString()} h ${minutes.toString()} min` : `${hours.toString()} h`;
  }
  return minutes > 0 ? `${minutes.toString()} min` : `${secs.toString()} s`;
}

/** Energy in watt-hours with about three significant digits (matches the CLI). */
export function formatEnergy(joules: number): string {
  const wh = joulesToWattHours(joules);
  if (wh >= 10) {
    return `${wh.toFixed(1)} Wh`;
  }
  return wh >= 0.1 ? `${wh.toFixed(2)} Wh` : `${wh.toFixed(3)} Wh`;
}
