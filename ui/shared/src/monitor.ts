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
