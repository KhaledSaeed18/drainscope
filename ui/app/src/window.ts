import Adw from 'gi://Adw?version=1';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gtk from 'gi://Gtk?version=4.0';

import {
  buildSleep,
  buildUsage,
  formatDuration,
  formatPercent,
  RANGES,
  rangeQuery,
  type Range,
  type Summary,
} from '@drainscope/shared';

import { appName, consumerIcon } from './apps';
import type { MonitorClient } from './client';

const SLEEP_HISTORY_SECONDS = 30 * 86_400;

function nowSeconds(): number {
  return Math.floor(GLib.get_real_time() / 1_000_000);
}

function removeAll(group: Adw.PreferencesGroup, rows: Gtk.Widget[]): void {
  for (const row of rows) {
    group.remove(row);
  }
  rows.length = 0;
}

/** Shows `Monitor1` history: usage per consumer or kind over a range, and sleep sessions. */
export class DrainscopeWindow extends Adw.ApplicationWindow {
  static {
    GObject.registerClass(this);
  }

  private readonly client: MonitorClient;
  private range: Range = 'unplug';
  private byKind = false;
  private refreshing = false;
  private pending = false;

  private readonly stack = new Gtk.Stack();
  private readonly status = new Adw.StatusPage({ icon_name: 'battery-missing-symbolic' });
  private readonly banner = new Adw.Banner();
  private readonly usageGroup = new Adw.PreferencesGroup();
  private readonly usageRows: Gtk.Widget[] = [];
  private readonly sleepGroup = new Adw.PreferencesGroup({ title: 'Sleep' });
  private readonly sleepRows: Gtk.Widget[] = [];

  constructor(application: Adw.Application, client: MonitorClient) {
    super({ application, title: 'drainscope', default_width: 520, default_height: 680 });
    this.client = client;

    const rangeChoice = Gtk.DropDown.new_from_strings(RANGES.map((r) => r.label));
    rangeChoice.connect('notify::selected', () => {
      this.range = RANGES[rangeChoice.get_selected()]?.id ?? 'unplug';
      this.refresh();
    });
    const kindToggle = new Gtk.ToggleButton({
      icon_name: 'view-list-symbolic',
      tooltip_text: 'Group by kind',
    });
    kindToggle.connect('toggled', () => {
      this.byKind = kindToggle.get_active();
      this.refresh();
    });

    const header = new Adw.HeaderBar();
    header.pack_start(rangeChoice);
    header.pack_end(kindToggle);

    const page = new Adw.PreferencesPage();
    page.add(this.usageGroup);
    page.add(this.sleepGroup);

    this.stack.add_named(page, 'history');
    this.stack.add_named(this.status, 'status');

    const toolbar = new Adw.ToolbarView({ content: this.stack });
    toolbar.add_top_bar(header);
    toolbar.add_top_bar(this.banner);
    this.set_content(toolbar);

    this.client.subscribeTicks(() => {
      this.refresh();
    });
    this.connect('close-request', () => {
      this.client.destroy();
      return false;
    });
    this.refresh();
  }

  /** Coalesces refreshes: ticks can arrive while a refresh is still waiting for replies. */
  private refresh(): void {
    if (this.refreshing) {
      this.pending = true;
      return;
    }
    this.refreshing = true;
    void this.load().finally(() => {
      this.refreshing = false;
      if (this.pending) {
        this.pending = false;
        this.refresh();
      }
    });
  }

  private showStatus(title: string, description: string): void {
    this.status.set_title(title);
    this.status.set_description(description);
    this.stack.set_visible_child_name('status');
  }

  private async load(): Promise<void> {
    const summary = await this.client.summary();
    if (!summary.ok) {
      this.showStatus(
        'drainscope isn’t running',
        `Start it with <tt>systemctl --user enable --now drainscope.service</tt>\n\n${GLib.markup_escape_text(summary.error, -1)}`,
      );
      return;
    }
    this.stack.set_visible_child_name('history');
    this.updateBanner(summary.value);
    const now = nowSeconds();
    await Promise.all([this.loadUsage(summary.value, now), this.loadSleep(now)]);
  }

  private updateBanner(summary: Summary): void {
    // batteryPercent is the share of the battery used since unplugging, not its level.
    const used = formatPercent(summary.batteryPercent);
    if (summary.onBattery) {
      const since = formatDuration(nowSeconds() - summary.sinceUnplug);
      this.banner.set_title(`On battery for ${since} · ${used} used`);
    } else if (summary.sinceUnplug > 0) {
      this.banner.set_title(`Plugged in · the last discharge used ${used}`);
    } else {
      this.banner.set_title('Plugged in');
    }
    this.banner.set_revealed(true);
  }

  private async loadUsage(summary: Summary, now: number): Promise<void> {
    const query = rangeQuery(this.range, now, summary.sinceUnplug);
    removeAll(this.usageGroup, this.usageRows);
    this.usageGroup.set_title('Usage');
    if (query === undefined) {
      this.usageGroup.set_description('The charger hasn’t been unplugged since drainscope started.');
      return;
    }
    const groupBy = this.byKind ? 'kind' : 'consumer';
    const [usage, coverage] = await Promise.all([
      this.client.usage(query.since, query.until, groupBy, query.source),
      this.client.coverage(query.since, query.until, query.source),
    ]);
    // A newer refresh may have started rendering; drop rows added meanwhile.
    removeAll(this.usageGroup, this.usageRows);
    if (!usage.ok) {
      this.usageGroup.set_description(GLib.markup_escape_text(usage.error, -1));
      return;
    }
    const span = query.until - query.since;
    // Daemons older than GetCoverage: average over the whole span.
    const measured = coverage.ok ? coverage.value : span;
    const model = buildUsage(usage.value, span, measured, this.byKind, appName);
    this.usageGroup.set_description(model.entries.length === 0 ? 'Nothing measured in this range yet.' : model.footer);
    for (const entry of model.entries) {
      const row = new Adw.ActionRow({
        title: GLib.markup_escape_text(entry.label, -1),
        subtitle: `${entry.energy} · ${entry.average}`,
      });
      row.add_prefix(new Gtk.Image({ gicon: consumerIcon(entry.consumer), pixel_size: 32 }));
      const share = new Gtk.Box({ orientation: Gtk.Orientation.VERTICAL, valign: Gtk.Align.CENTER, spacing: 4 });
      share.append(new Gtk.Label({ label: entry.share, xalign: 1, css_classes: ['numeric'] }));
      share.append(new Gtk.ProgressBar({ fraction: entry.fraction, width_request: 80 }));
      row.add_suffix(share);
      this.usageGroup.add(row);
      this.usageRows.push(row);
    }
  }

  private async loadSleep(now: number): Promise<void> {
    const sessions = await this.client.sleepSessions(now - SLEEP_HISTORY_SECONDS);
    removeAll(this.sleepGroup, this.sleepRows);
    if (!sessions.ok) {
      this.sleepGroup.set_description(GLib.markup_escape_text(sessions.error, -1));
      return;
    }
    const entries = buildSleep(sessions.value, now);
    this.sleepGroup.set_description(entries.length === 0 ? 'No suspends in the last 30 days.' : '');
    for (const entry of entries) {
      const row = new Adw.ActionRow({ title: entry.title, subtitle: entry.subtitle });
      row.add_prefix(new Gtk.Image({ icon_name: 'weather-clear-night-symbolic' }));
      this.sleepGroup.add(row);
      this.sleepRows.push(row);
    }
  }
}
