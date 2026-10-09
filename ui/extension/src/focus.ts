import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { focusAppId } from '@drainscope/shared';

import { BUS_NAME, CALL_TIMEOUT_MS, INTERFACE, OBJECT_PATH } from './client';

// GNOME Shell already promisifies Gio.DBusConnection.prototype.call.

function isLocked(): boolean {
  // Typed `any` in @girs; narrow before reading.
  const mode: unknown = Main.sessionMode;
  return typeof mode === 'object' && mode !== null && 'isLocked' in mode && mode.isLocked === true;
}

/**
 * Tells the daemon which app has focus (ADR 0011): only its desktop ID, on the session bus.
 * The daemon splits each app's energy into foreground and background with it.
 */
export class FocusReporter {
  private readonly tracker = Shell.WindowTracker.get_default();
  private focusSignal: number | null = null;
  private ownerSubscription: number | null = null;
  private sent: string | null = null;

  start(): void {
    this.focusSignal = this.tracker.connect('notify::focus-app', () => {
      this.report(false);
    });
    // A restarted daemon starts with focus unknown: tell it again.
    this.ownerSubscription = Gio.DBus.session.signal_subscribe(
      'org.freedesktop.DBus',
      'org.freedesktop.DBus',
      'NameOwnerChanged',
      '/org/freedesktop/DBus',
      BUS_NAME,
      Gio.DBusSignalFlags.NONE,
      (_connection, _sender, _path, _interface, _signal, parameters) => {
        if (parameters.get_type_string() === '(sss)' && parameters.get_child_value(2).get_string()[0] !== '') {
          this.report(true);
        }
      },
    );
    this.report(true);
  }

  /** Stops reporting. Locking disables extensions: then nothing has focus; otherwise unknown. */
  stop(): void {
    if (this.focusSignal !== null) {
      this.tracker.disconnect(this.focusSignal);
      this.focusSignal = null;
    }
    if (this.ownerSubscription !== null) {
      Gio.DBus.session.signal_unsubscribe(this.ownerSubscription);
      this.ownerSubscription = null;
    }
    if (isLocked()) {
      this.call('SetFocus', new GLib.Variant('(s)', ['']));
    } else {
      this.call('EndFocus', null);
    }
    this.sent = null;
  }

  private report(force: boolean): void {
    const appId = focusAppId(this.tracker.focus_app?.get_id() ?? null);
    if (!force && appId === this.sent) {
      return;
    }
    this.sent = appId;
    this.call('SetFocus', new GLib.Variant('(s)', [appId]));
  }

  private call(method: string, parameters: GLib.Variant | null): void {
    // Fire and forget, never cancelled: stop() must still reach the daemon. A missing daemon
    // only means nobody records focus.
    Gio.DBus.session
      .call(BUS_NAME, OBJECT_PATH, INTERFACE, method, parameters, null, Gio.DBusCallFlags.NO_AUTO_START, CALL_TIMEOUT_MS, null)
      .catch((error: unknown) => {
        console.debug(`drainscope: ${method} failed: ${error instanceof Error ? error.message : String(error)}`);
      });
  }
}
