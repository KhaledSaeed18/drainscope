import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {
  COVERAGE_SIGNATURE,
  decodeCoverage,
  decodeBatteryHealth,
  decodeSleepHistory,
  decodeSummary,
  decodeNetwork,
  decodeUsage,
  decodeWakeups,
  HEALTH_SIGNATURE,
  NETWORK_SIGNATURE,
  SLEEP_SIGNATURE,
  SUMMARY_SIGNATURE,
  USAGE_SIGNATURE,
  WAKEUPS_SIGNATURE,
  type Decoded,
  type HealthReading,
  type Network,
  type SleepSession,
  type Summary,
  type UsageRow,
  type Wakeups,
} from '@drainscope/shared';

const BUS_NAME = 'io.github.khaledsaeed18.Drainscope.Monitor';
const OBJECT_PATH = '/io/github/khaledsaeed18/Drainscope/Monitor';
const INTERFACE = 'io.github.khaledsaeed18.Drainscope.Monitor1';
const CALL_TIMEOUT_MS = 5000;

Gio._promisify(Gio.DBusConnection.prototype, 'call');

export type Result<T> = { ok: true; value: T } | { ok: false; error: string };

/** The daemon's `Monitor1` interface on the session bus. */
export class MonitorClient {
  private readonly cancellable = new Gio.Cancellable();
  private tickSubscription = 0;

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

  private async call<T>(
    method: string,
    parameters: GLib.Variant | null,
    signature: string,
    decode: (value: unknown) => Decoded<T>,
  ): Promise<Result<T>> {
    try {
      const reply = await Gio.DBus.session.call(
        BUS_NAME,
        OBJECT_PATH,
        INTERFACE,
        method,
        parameters,
        new GLib.VariantType(signature),
        Gio.DBusCallFlags.NONE,
        CALL_TIMEOUT_MS,
        this.cancellable,
      );
      if (reply.get_type_string() !== signature) {
        return { ok: false, error: `unexpected reply ${reply.get_type_string()}` };
      }
      const unpacked: unknown = reply.recursiveUnpack();
      const decoded = decode(unpacked);
      return decoded.ok ? decoded : { ok: false, error: decoded.error };
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes('UnknownMethod')) {
        return { ok: false, error: 'Needs a newer drainscope daemon; update and restart it.' };
      }
      return { ok: false, error: message };
    }
  }

  summary(): Promise<Result<Summary>> {
    return this.call('GetSummary', null, SUMMARY_SIGNATURE, decodeSummary);
  }

  usage(since: number, until: number, groupBy: 'consumer' | 'kind', source: string): Promise<Result<UsageRow[]>> {
    const parameters = new GLib.Variant('(xxss)', [since, until, groupBy, source]);
    return this.call('GetUsage', parameters, USAGE_SIGNATURE, decodeUsage);
  }

  coverage(since: number, until: number, source: string): Promise<Result<number>> {
    const parameters = new GLib.Variant('(xxs)', [since, until, source]);
    return this.call('GetCoverage', parameters, COVERAGE_SIGNATURE, decodeCoverage);
  }

  sleepSessions(since: number): Promise<Result<SleepSession[]>> {
    const parameters = new GLib.Variant('(x)', [since]);
    return this.call('GetSleepHistory', parameters, SLEEP_SIGNATURE, decodeSleepHistory);
  }

  network(): Promise<Result<Network>> {
    return this.call('GetNetwork', null, NETWORK_SIGNATURE, decodeNetwork);
  }

  wakeups(): Promise<Result<Wakeups>> {
    return this.call('GetWakeups', null, WAKEUPS_SIGNATURE, decodeWakeups);
  }

  batteryHealth(since: number): Promise<Result<HealthReading[]>> {
    const parameters = new GLib.Variant('(x)', [since]);
    return this.call('GetBatteryHealth', parameters, HEALTH_SIGNATURE, decodeBatteryHealth);
  }

  destroy(): void {
    if (this.tickSubscription !== 0) {
      Gio.DBus.session.signal_unsubscribe(this.tickSubscription);
      this.tickSubscription = 0;
    }
    this.cancellable.cancel();
  }
}
