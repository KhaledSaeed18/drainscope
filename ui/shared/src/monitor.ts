/**
 * Typed views of `Monitor1` replies (data/dbus/interfaces/…Monitor1.xml).
 *
 * The extension unpacks a GLib.Variant into plain JS values after checking its type string;
 * these decoders then validate the shape at runtime, so nothing downstream sees `any`.
 */

/** D-Bus type of `GetSummary`'s reply. */
export const SUMMARY_SIGNATURE = '(bxda(sddd))';

export interface TopConsumer {
  /** Storage key, e.g. `app:org.mozilla.firefox`. */
  key: string;
  joules: number;
  /** Percent of battery capacity. */
  ofBattery: number;
  /** Percent of attributable (non-idle, non-devices) energy; 0 for the rest. */
  ofActive: number;
}

export interface Summary {
  onBattery: boolean;
  /** Unix seconds when the current or last discharge started; 0 if never unplugged. */
  sinceUnplug: number;
  batteryPercent: number;
  top: TopConsumer[];
}

export type Decoded<T> = { ok: true; value: T } | { ok: false; error: string };

function isArray(value: unknown): value is readonly unknown[] {
  return Array.isArray(value);
}

function isNumber(value: unknown): value is number {
  return typeof value === 'number';
}

function decodeTop(value: unknown): TopConsumer | undefined {
  if (!isArray(value) || value.length !== 4) {
    return undefined;
  }
  const [key, joules, ofBattery, ofActive] = value;
  if (typeof key !== 'string' || !isNumber(joules) || !isNumber(ofBattery) || !isNumber(ofActive)) {
    return undefined;
  }
  return { key, joules, ofBattery, ofActive };
}

/** Decodes the unpacked `(bxda(sddd))` reply of `GetSummary`. */
export function decodeSummary(value: unknown): Decoded<Summary> {
  if (!isArray(value) || value.length !== 4) {
    return { ok: false, error: 'expected a 4-tuple' };
  }
  const [onBattery, sinceUnplug, batteryPercent, rows] = value;
  if (typeof onBattery !== 'boolean' || !isNumber(sinceUnplug) || !isNumber(batteryPercent) || !isArray(rows)) {
    return { ok: false, error: 'unexpected field types' };
  }
  const top: TopConsumer[] = [];
  for (const row of rows) {
    const decoded = decodeTop(row);
    if (decoded === undefined) {
      return { ok: false, error: 'malformed consumer row' };
    }
    top.push(decoded);
  }
  return { ok: true, value: { onBattery, sinceUnplug, batteryPercent, top } };
}

/** D-Bus type of `GetUsage`'s reply. */
export const USAGE_SIGNATURE = '(a(ssdddd))';
/** D-Bus type of `GetCoverage`'s reply. */
export const COVERAGE_SIGNATURE = '(t)';
/** D-Bus type of `GetSleepHistory`'s reply. */
export const SLEEP_SIGNATURE = '(a(xxddss))';
/** D-Bus type of `GetWakeups`' reply. */
export const WAKEUPS_SIGNATURE = '(ba(sd))';
/** D-Bus type of `GetBatteryHealth`'s reply. */
export const HEALTH_SIGNATURE = '(a(sxddu))';

export interface UsageRow {
  key: string;
  kind: string;
  total: number;
  cpu: number;
  gpu: number;
  other: number;
}

export interface SleepSession {
  /** Unix seconds. */
  start: number;
  end: number;
  /** NaN when unknown. */
  whLost: number;
  percentLost: number;
  /** e.g. `deep`; empty when unknown. */
  mode: string;
  /** e.g. `Lid (PNP0C0D:00)`; empty when unknown. */
  wakeReason: string;
}

export interface HealthReading {
  battery: string;
  /** Unix seconds. */
  time: number;
  fullWh: number;
  /** NaN when unknown. */
  designWh: number;
  /** 0 when unknown. */
  cycles: number;
}

/** Unwraps a one-element reply tuple. */
function single(value: unknown): unknown {
  return isArray(value) && value.length === 1 ? value[0] : undefined;
}

/** Decodes the unpacked `(a(ssdddd))` reply of `GetUsage`. */
export function decodeUsage(value: unknown): Decoded<UsageRow[]> {
  const rows = single(value);
  if (!isArray(rows)) {
    return { ok: false, error: 'expected (a(ssdddd))' };
  }
  const usage: UsageRow[] = [];
  for (const row of rows) {
    if (!isArray(row) || row.length !== 6) {
      return { ok: false, error: 'malformed usage row' };
    }
    const [key, kind, total, cpu, gpu, other] = row;
    if (
      typeof key !== 'string' ||
      typeof kind !== 'string' ||
      !isNumber(total) ||
      !isNumber(cpu) ||
      !isNumber(gpu) ||
      !isNumber(other)
    ) {
      return { ok: false, error: 'malformed usage row' };
    }
    usage.push({ key, kind, total, cpu, gpu, other });
  }
  return { ok: true, value: usage };
}

/** Decodes the unpacked `(t)` reply of `GetCoverage`: measured seconds. */
export function decodeCoverage(value: unknown): Decoded<number> {
  const seconds = single(value);
  return isNumber(seconds) ? { ok: true, value: seconds } : { ok: false, error: 'expected (t)' };
}

/** Decodes the unpacked `(a(xxddss))` reply of `GetSleepHistory`. */
export function decodeSleepHistory(value: unknown): Decoded<SleepSession[]> {
  const rows = single(value);
  if (!isArray(rows)) {
    return { ok: false, error: 'expected (a(xxddss))' };
  }
  const sessions: SleepSession[] = [];
  for (const row of rows) {
    if (!isArray(row) || row.length !== 6) {
      return { ok: false, error: 'malformed sleep session' };
    }
    const [start, end, whLost, percentLost, mode, wakeReason] = row;
    if (
      !isNumber(start) ||
      !isNumber(end) ||
      !isNumber(whLost) ||
      !isNumber(percentLost) ||
      typeof mode !== 'string' ||
      typeof wakeReason !== 'string'
    ) {
      return { ok: false, error: 'malformed sleep session' };
    }
    sessions.push({ start, end, whLost, percentLost, mode, wakeReason });
  }
  return { ok: true, value: sessions };
}

/** Decodes the unpacked `(a(sxddu))` reply of `GetBatteryHealth`. */
export function decodeBatteryHealth(value: unknown): Decoded<HealthReading[]> {
  const rows = single(value);
  if (!isArray(rows)) {
    return { ok: false, error: 'expected (a(sxddu))' };
  }
  const readings: HealthReading[] = [];
  for (const row of rows) {
    if (!isArray(row) || row.length !== 5) {
      return { ok: false, error: 'malformed health reading' };
    }
    const [battery, time, fullWh, designWh, cycles] = row;
    if (typeof battery !== 'string' || !isNumber(time) || !isNumber(fullWh) || !isNumber(designWh) || !isNumber(cycles)) {
      return { ok: false, error: 'malformed health reading' };
    }
    readings.push({ battery, time, fullWh, designWh, cycles });
  }
  return { ok: true, value: readings };
}

export interface Wakeups {
  /** False without the eBPF probe. */
  available: boolean;
  /** (key, idle exits per second), largest first. */
  rates: { key: string; perSecond: number }[];
}

/** Decodes the unpacked `(ba(sd))` reply of `GetWakeups`. */
export function decodeWakeups(value: unknown): Decoded<Wakeups> {
  if (!isArray(value) || value.length !== 2) {
    return { ok: false, error: 'expected (ba(sd))' };
  }
  const [available, rows] = value;
  if (typeof available !== 'boolean' || !isArray(rows)) {
    return { ok: false, error: 'expected (ba(sd))' };
  }
  const rates: Wakeups['rates'] = [];
  for (const row of rows) {
    if (!isArray(row) || row.length !== 2) {
      return { ok: false, error: 'malformed wakeup row' };
    }
    const [key, perSecond] = row;
    if (typeof key !== 'string' || !isNumber(perSecond)) {
      return { ok: false, error: 'malformed wakeup row' };
    }
    rates.push({ key, perSecond });
  }
  return { ok: true, value: { available, rates } };
}
