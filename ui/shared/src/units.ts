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
  return `${Math.round(percent).toString()}%`;
}
