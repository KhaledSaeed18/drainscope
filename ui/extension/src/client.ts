import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import { decodeSummary, SUMMARY_SIGNATURE, type Summary } from '@drainscope/shared';

const BUS_NAME = 'io.github.khaledsaeed18.Drainscope.Monitor';
const OBJECT_PATH = '/io/github/khaledsaeed18/Drainscope/Monitor';
const INTERFACE = 'io.github.khaledsaeed18.Drainscope.Monitor1';
const CALL_TIMEOUT_MS = 5000;

// GNOME Shell already promisifies Gio.DBusConnection.prototype.call.

export type SummaryResult =
  | { kind: 'summary'; summary: Summary }
  | { kind: 'unavailable'; reason: string };

/** A small client for the daemon's `Monitor1` interface on the session bus. */
export class MonitorClient {
  private readonly cancellable = new Gio.Cancellable();
  private tickSubscription: number | null = null;

  async summary(): Promise<SummaryResult> {
    try {
      const reply = await Gio.DBus.session.call(
        BUS_NAME,
        OBJECT_PATH,
        INTERFACE,
        'GetSummary',
        null,
        new GLib.VariantType(SUMMARY_SIGNATURE),
        Gio.DBusCallFlags.NONE,
        CALL_TIMEOUT_MS,
        this.cancellable,
      );
      if (reply.get_type_string() !== SUMMARY_SIGNATURE) {
        return { kind: 'unavailable', reason: `unexpected reply ${reply.get_type_string()}` };
      }
      const unpacked: unknown = reply.recursiveUnpack();
      const decoded = decodeSummary(unpacked);
      return decoded.ok
        ? { kind: 'summary', summary: decoded.value }
        : { kind: 'unavailable', reason: decoded.error };
    } catch (error: unknown) {
      return { kind: 'unavailable', reason: error instanceof Error ? error.message : String(error) };
    }
  }

  /** Calls `onTick` after every attribution tick of the daemon (every few seconds). */
  subscribeTicks(onTick: () => void): void {
    this.tickSubscription = Gio.DBus.session.signal_subscribe(
      BUS_NAME,
      INTERFACE,
      'Tick',
      OBJECT_PATH,
      null,
      Gio.DBusSignalFlags.NONE,
      () => {
        onTick();
      },
    );
  }

  destroy(): void {
    this.cancellable.cancel();
    if (this.tickSubscription !== null) {
      Gio.DBus.session.signal_unsubscribe(this.tickSubscription);
      this.tickSubscription = null;
    }
  }
}
