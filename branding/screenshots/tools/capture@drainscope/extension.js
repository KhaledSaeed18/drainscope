// Screenshot sessions only (session.sh). Follows the shot plan in $CAPTURE_PLAN, a JSON list of
// fixed actions, and saves full-stage screenshots to $CAPTURE_DIR with the geometry of the app
// window and the open menus, so compose.py can crop them exactly. Writes $CAPTURE_DIR/../done.
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import St from 'gi://St';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const APP_ID = 'io.github.khaledsaeed18.Drainscope.desktop';

const wait = (seconds) =>
  new Promise((resolve) => {
    GLib.timeout_add(GLib.PRIORITY_DEFAULT, Math.round(seconds * 1000), () => {
      resolve();
      return GLib.SOURCE_REMOVE;
    });
  });

function extents(actor) {
  if (!actor?.visible) return null;
  const box = actor.get_transformed_extents();
  return [box.origin.x, box.origin.y, box.size.width, box.size.height].map(Math.round);
}

export default class Capture extends Extension {
  enable() {
    this._pointer = null;
    void this._run().catch((e) => console.error(`capture: ${e}\n${e.stack}`));
  }

  disable() {
    this._pointer = null;
  }

  get _toggle() {
    const grid = Main.panel.statusArea.quickSettings.menu._grid;
    return grid.get_children().find((c) => c.title === 'Battery' && c.gicon instanceof Gio.FileIcon);
  }

  get _window() {
    const app = Shell.AppSystem.get_default().lookup_app(APP_ID);
    return app?.get_windows()[0] ?? null;
  }

  async _run() {
    const dir = GLib.getenv('CAPTURE_DIR');
    const plan = JSON.parse(new TextDecoder().decode(GLib.file_get_contents(GLib.getenv('CAPTURE_PLAN'))[1]));
    const iface = new Gio.Settings({ schema_id: 'org.gnome.desktop.interface' });
    const background = new Gio.Settings({ schema_id: 'org.gnome.desktop.background' });
    const quick = Main.panel.statusArea.quickSettings;
    const geometry = {};
    for (const step of plan) {
      console.log(`capture: ${JSON.stringify(step)} (overview ${Main.overview.visible})`);
      switch (step.do) {
        case 'wait':
          await wait(step.seconds);
          break;
        case 'scheme':
          iface.set_string('color-scheme', step.value);
          await wait(1.5);
          break;
        case 'background':
          background.set_string('picture-uri', step.uri);
          background.set_string('picture-uri-dark', step.uri);
          await wait(1);
          break;
        case 'hide-overview':
          Main.overview.hide();
          await wait(1);
          break;
        case 'tidy': {
          // Real values leak in from the host's system bus: the Wi-Fi network name and the
          // battery level. Replace them with neutral ones that match the demo data.
          const labels = [];
          const walk = (actor) => {
            if (actor instanceof St.Label) labels.push(actor);
            for (const child of actor.get_children()) walk(child);
          };
          walk(quick.menu.box);
          for (const label of labels) {
            if (/^\d+\s?%$/.test(label.text)) label.text = step.battery;
          }
          for (const item of quick.menu._grid.get_children()) {
            if (item.title === 'Wi-Fi') item.subtitle = step.wifi;
          }
          await wait(0.5);
          break;
        }
        case 'quick-settings':
          quick.menu.open();
          await wait(1);
          break;
        case 'tile-menu':
          this._toggle?.menu.open();
          await wait(1);
          break;
        case 'close-menus':
          this._toggle?.menu.close();
          quick.menu.close();
          await wait(1);
          break;
        case 'launch-app': {
          Shell.AppSystem.get_default().lookup_app(APP_ID).activate();
          for (let i = 0; i < 50 && this._window === null; i++) await wait(0.2);
          await wait(step.settle ?? 3);
          break;
        }
        case 'place-app': {
          const win = this._window;
          win?.unmaximize?.();
          win?.move_resize_frame(false, step.x, step.y, step.width, step.height);
          await wait(1);
          break;
        }
        case 'close-app':
          this._window?.delete(global.get_current_time());
          await wait(1.5);
          break;
        case 'move':
        case 'click':
        case 'scroll': {
          this._pointer ??= Clutter.get_default_backend()
            .get_default_seat()
            .create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
          const time = () => GLib.get_monotonic_time();
          // Two motions: the first after the device appears doesn't always reach the window.
          this._pointer.notify_absolute_motion(time(), step.x - 2, step.y - 2);
          await wait(0.3);
          this._pointer.notify_absolute_motion(time(), step.x, step.y);
          await wait(0.3);
          if (step.do === 'move') {
            await wait(step.settle ?? 0.5);
            break;
          }
          if (step.do === 'click') {
            this._pointer.notify_button(time(), Clutter.BUTTON_PRIMARY, Clutter.ButtonState.PRESSED);
            await wait(0.08);
            this._pointer.notify_button(time(), Clutter.BUTTON_PRIMARY, Clutter.ButtonState.RELEASED);
          } else {
            // Wheel steps: deterministic, unlike kinetic touchpad scrolling.
            const direction = step.steps < 0 ? Clutter.ScrollDirection.UP : Clutter.ScrollDirection.DOWN;
            for (let i = 0; i < Math.abs(step.steps ?? 0); i++) {
              this._pointer.notify_discrete_scroll(time(), direction, Clutter.ScrollSource.WHEEL);
              await wait(0.05);
            }
            // A smooth, non-kinetic nudge for fine positioning.
            if (step.nudge) {
              this._pointer.notify_scroll_continuous(time(), 0, step.nudge, Clutter.ScrollSource.CONTINUOUS, Clutter.ScrollFinishFlags.NONE);
            }
          }
          // Park the pointer in a corner so no hover highlight shows in the next shot.
          await wait(0.2);
          this._pointer.notify_absolute_motion(time(), 1, global.stage.height - 1);
          await wait(step.settle ?? 1.2);
          break;
        }
        case 'shot': {
          const file = Gio.File.new_for_path(`${dir}/${step.name}.png`);
          const stream = file.replace(null, false, Gio.FileCreateFlags.NONE, null);
          await new Shell.Screenshot().screenshot(step.cursor ?? false, stream);
          stream.close(null);
          const win = this._window;
          const frame = win?.get_frame_rect();
          geometry[step.name] = {
            scale: global.display.get_monitor_scale(0),
            window: frame ? [frame.x, frame.y, frame.width, frame.height] : null,
            quickSettings: quick.menu.isOpen ? extents(quick.menu.box) : null,
            tileMenu: this._toggle?.menu.isOpen ? extents(this._toggle.menu.box) : null,
          };
          break;
        }
      }
    }
    GLib.file_set_contents(`${dir}/geometry.json`, JSON.stringify(geometry, null, 2));
    GLib.file_set_contents(`${dir}/../done`, '');
  }
}
