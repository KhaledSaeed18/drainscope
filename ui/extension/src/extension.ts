/**
 * drainscope GNOME Shell extension: a "Battery usage" quick-settings tile whose menu lists
 * what used the battery since the charger was unplugged. All data comes from the drainscope
 * daemon over D-Bus (Monitor1); the extension measures nothing itself.
 */

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import { QuickMenuToggle, SystemIndicator } from 'resource:///org/gnome/shell/ui/quickSettings.js';

import { buildMenu, iconName, type MenuModel, type MenuRow } from '@drainscope/shared';

import { appIcon, appName } from './apps';
import { MonitorClient } from './client';
import { FocusReporter } from './focus';

/** How often the tile's subtitle refreshes while the menu is closed. */
const BACKGROUND_REFRESH_SECONDS = 60;
const TITLE = 'Battery';
/** drainscope's symbolic app icon, shipped in the extension; St recolors `-symbolic.svg` files. */
const ICON_FILE = 'icons/io.github.khaledsaeed18.Drainscope-symbolic.svg';

function rowIcon(row: MenuRow): Gio.Icon {
  const fromApp = row.consumer.kind === 'app' ? appIcon(row.consumer.name) : null;
  return fromApp ?? Gio.ThemedIcon.new(iconName(row.consumer));
}

const UsageToggle = GObject.registerClass(
  class UsageToggle extends QuickMenuToggle {
    private readonly rows = new PopupMenu.PopupMenuSection();
    private onBattery = false;

    constructor(params: { gicon: Gio.Icon }) {
      super({ title: TITLE, subtitle: '…', gicon: params.gicon });
      this.menu.setHeader(this.gicon, TITLE, '');
      this.menu.addMenuItem(this.rows);
      this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
      const footer = new PopupMenu.PopupMenuItem('More detail: run drainscope in a terminal', {
        reactive: false,
      });
      footer.label.add_style_class_name('drainscope-footer');
      this.menu.addMenuItem(footer);
      // The tile has no on/off state of its own: it is lit while on battery, and clicking it
      // opens the menu instead of toggling.
      this.connect('clicked', () => {
        this.checked = this.onBattery;
        this.menu.open();
      });
    }

    showModel(model: MenuModel, onBattery: boolean): void {
      this.onBattery = onBattery;
      this.checked = onBattery;
      this.subtitle = model.subtitle;
      this.menu.setHeader(this.gicon, TITLE, model.headline);
      this.rows.removeAll();
      for (const row of model.rows) {
        this.rows.addMenuItem(rowItem(row));
      }
    }

    showUnavailable(): void {
      this.onBattery = false;
      this.checked = false;
      this.subtitle = 'Daemon not running';
      this.menu.setHeader(
        this.gicon,
        TITLE,
        'Start it with: systemctl --user enable --now drainscope.service',
      );
      this.rows.removeAll();
    }
  },
);

type UsageToggleInstance = InstanceType<typeof UsageToggle>;

function rowItem(row: MenuRow): PopupMenu.PopupImageMenuItem {
  const item = new PopupMenu.PopupImageMenuItem(row.label, rowIcon(row));
  // Informational rows: nothing happens on activation.
  item.reactive = false;
  item.label.add_style_class_name('drainscope-label');
  item.label.x_expand = true;
  item.add_child(
    new St.Label({
      text: row.energy,
      style_class: 'drainscope-energy',
      y_align: Clutter.ActorAlign.CENTER,
    }),
  );
  item.add_child(
    new St.Label({
      text: row.share,
      style_class: 'drainscope-share',
      y_align: Clutter.ActorAlign.CENTER,
    }),
  );
  return item;
}

export default class DrainscopeExtension extends Extension {
  private indicator: SystemIndicator | null = null;
  private toggle: UsageToggleInstance | null = null;
  private client: MonitorClient | null = null;
  private refreshSource: number | null = null;
  private menuSignal: number | null = null;
  private focus: FocusReporter | null = null;

  override enable(): void {
    const quickSettings = Main.panel.statusArea.quickSettings;
    if (quickSettings === undefined) {
      console.error('drainscope: quick settings unavailable');
      return;
    }
    const toggle = new UsageToggle({
      gicon: Gio.FileIcon.new(this.dir.resolve_relative_path(ICON_FILE)),
    });
    const indicator = new SystemIndicator();
    indicator.quickSettingsItems.push(toggle);
    quickSettings.addExternalIndicator(indicator);

    const client = new MonitorClient();
    // While the menu is open, follow the daemon's ticks; otherwise refresh slowly. Subscribing
    // only then keeps the Shell from waking for every tick while nobody looks.
    this.menuSignal = toggle.menu.connect('open-state-changed', (_menu, open) => {
      if (open) {
        client.subscribeTicks(() => {
          this.refresh();
        });
        this.refresh();
      } else {
        client.unsubscribeTicks();
      }
    });
    this.refreshSource = GLib.timeout_add_seconds(
      GLib.PRIORITY_LOW,
      BACKGROUND_REFRESH_SECONDS,
      () => {
        this.refresh();
        return GLib.SOURCE_CONTINUE;
      },
    );

    this.indicator = indicator;
    this.toggle = toggle;
    this.client = client;
    this.focus = new FocusReporter();
    this.focus.start();
    this.refresh();
  }

  override disable(): void {
    this.focus?.stop();
    this.focus = null;
    if (this.refreshSource !== null) {
      GLib.Source.remove(this.refreshSource);
      this.refreshSource = null;
    }
    if (this.menuSignal !== null && this.toggle !== null) {
      this.toggle.menu.disconnect(this.menuSignal);
      this.menuSignal = null;
    }
    this.client?.destroy();
    this.client = null;
    for (const item of this.indicator?.quickSettingsItems ?? []) {
      item.destroy();
    }
    this.indicator?.destroy();
    this.indicator = null;
    this.toggle = null;
  }

  private refresh(): void {
    const client = this.client;
    if (client === null) {
      return;
    }
    void client.summary().then((result) => {
      // Disabled while the call was in flight.
      const toggle = this.toggle;
      if (toggle === null) {
        return;
      }
      if (result.kind === 'unavailable') {
        toggle.showUnavailable();
        return;
      }
      const nowSeconds = GLib.DateTime.new_now_utc().to_unix();
      toggle.showModel(buildMenu(result.summary, appName, nowSeconds), result.summary.onBattery);
    });
  }
}
