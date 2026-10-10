import Adw from 'gi://Adw?version=1';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gtk from 'gi://Gtk?version=4.0';

import {
  buildHealth,
  buildNetwork,
  buildSleep,
  buildTimeline,
  buildUsage,
  buildWakeups,
  formatDuration,
  formatPercent,
  RANGES,
  rangeQuery,
  type Range,
  type Summary,
  timelineBuckets,
  type HealthEntry,
  type NetworkEntry,
  type SleepEntry,
  type UsageEntry,
  type UsageRow,
  type WakeupEntry,
} from '@drainscope/shared';

import { appName, consumerIcon } from './apps';
import { axisStart, legend, TimelineChart } from './chart';
import type { MonitorClient } from './client';
import { detailPage } from './detail';
import { type BoundRow, dataRow, RowList } from './rows';

const SLEEP_HISTORY_SECONDS = 30 * 86_400;
const WAKEUP_ROWS = 5;
const HEALTH_HISTORY_SECONDS = 365 * 86_400;
/** Health changes daily; reload it at most this often. */
const HEALTH_REFRESH_SECONDS = 3600;
/** The timeline makes a query per bar, so it reloads at most this often on ticks. */
const TIMELINE_REFRESH_SECONDS = 60;

function nowSeconds(): number {
  return Math.floor(GLib.get_real_time() / 1_000_000);
}

/** A right-aligned value above a bar, as a row suffix. */
function valueBar(): { box: Gtk.Box; label: Gtk.Label; bar: Gtk.ProgressBar } {
  const box = new Gtk.Box({ orientation: Gtk.Orientation.VERTICAL, valign: Gtk.Align.CENTER, spacing: 4 });
  const label = new Gtk.Label({ xalign: 1, css_classes: ['numeric'] });
  const bar = new Gtk.ProgressBar({ width_request: 80 });
  box.append(label);
  box.append(bar);
  return { box, label, bar };
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

  private readonly navigation = new Adw.NavigationView();
  private readonly stack = new Gtk.Stack();
  private readonly status = new Adw.StatusPage({ icon_name: 'battery-missing-symbolic' });
  private readonly banner = new Adw.Banner();
  private readonly timelineGroup = new Adw.PreferencesGroup({ title: 'Timeline' });
  private readonly chart = new TimelineChart();
  private readonly axisStart = new Gtk.Label({ xalign: 0, hexpand: true, css_classes: ['caption', 'dim-label'] });
  private readonly axisEnd = new Gtk.Label({ xalign: 1, css_classes: ['caption', 'dim-label'] });
  private readonly legendSlot = new Gtk.Box();
  /** Range and time of the last timeline load. */
  private timelineLoaded: { range: Range; at: number } | undefined;
  private readonly usageGroup = new Adw.PreferencesGroup({ title: 'Usage' });
  private readonly usageRows = new RowList<UsageEntry>(
    // Group rows (by kind, or the folded tail) have no consumer; their keys can equal a
    // consumer's (`devices`), and they show a different icon.
    (entry) => `${entry.consumer === undefined ? 'group' : 'consumer'}/${entry.key}`,
    (entry) => this.usageRow(entry),
  );
  private readonly wakeupsGroup = new Adw.PreferencesGroup({
    title: 'Waking the processor',
    description: 'Times per second over the last minute. Frequent wakeups drain the battery even when little else happens.',
    visible: false,
  });
  private readonly wakeupRows = new RowList<WakeupEntry>(
    (entry) => entry.key,
    (entry) => this.wakeupRow(entry),
  );
  private readonly networkGroup = new Adw.PreferencesGroup({
    title: 'Network',
    description: 'Average over the last minute. Network traffic keeps the Wi-Fi radio awake.',
    visible: false,
  });
  private readonly networkRows = new RowList<NetworkEntry>(
    (entry) => entry.key,
    (entry) => this.networkRow(entry),
  );
  private readonly sleepGroup = new Adw.PreferencesGroup({ title: 'Sleep' });
  private readonly sleepRows = new RowList<SleepEntry>(
    (entry) => entry.key,
    (entry) => {
      const row = dataRow(entry.title, entry.subtitle);
      row.add_prefix(new Gtk.Image({ icon_name: 'weather-clear-night-symbolic' }));
      return { row, update: (e) => { row.set_title(e.title); row.set_subtitle(e.subtitle); } };
    },
  );
  private readonly healthGroup = new Adw.PreferencesGroup({ title: 'Battery health' });
  private readonly healthRows = new RowList<HealthEntry>(
    (entry) => entry.battery,
    () => this.healthRow(),
  );
  private healthLoadedAt: number | undefined;

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

    const axis = new Gtk.Box();
    axis.append(this.axisStart);
    axis.append(this.axisEnd);
    const timeline = new Gtk.Box({ orientation: Gtk.Orientation.VERTICAL, spacing: 6 });
    timeline.append(this.chart);
    timeline.append(axis);
    timeline.append(this.legendSlot);
    this.timelineGroup.add(timeline);

    this.usageGroup.add(this.usageRows.widget);
    this.wakeupsGroup.add(this.wakeupRows.widget);
    this.networkGroup.add(this.networkRows.widget);
    this.sleepGroup.add(this.sleepRows.widget);
    this.healthGroup.add(this.healthRows.widget);

    const page = new Adw.PreferencesPage();
    page.add(this.timelineGroup);
    page.add(this.usageGroup);
    page.add(this.wakeupsGroup);
    page.add(this.networkGroup);
    page.add(this.sleepGroup);
    page.add(this.healthGroup);

    this.stack.add_named(page, 'history');
    this.stack.add_named(this.status, 'status');

    const toolbar = new Adw.ToolbarView({ content: this.stack });
    toolbar.add_top_bar(header);
    toolbar.add_top_bar(this.banner);
    this.navigation.add(new Adw.NavigationPage({ title: 'drainscope', tag: 'history', child: toolbar }));
    this.set_content(this.navigation);

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
    await Promise.all([
      this.loadTimeline(summary.value, now),
      this.loadUsage(summary.value, now),
      this.loadWakeups(),
      this.loadNetwork(),
      this.loadSleep(now),
      this.loadHealth(now),
    ]);
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

  private async loadTimeline(summary: Summary, now: number): Promise<void> {
    const range = this.range;
    const loaded = this.timelineLoaded;
    if (loaded?.range === range && now - loaded.at < TIMELINE_REFRESH_SECONDS) {
      return;
    }
    const query = rangeQuery(range, now, summary.sinceUnplug);
    this.timelineGroup.set_visible(query !== undefined);
    if (query === undefined) {
      return;
    }
    this.timelineLoaded = { range, at: now };
    const buckets = timelineBuckets(range, query.since, query.until);
    const replies = await Promise.all(
      buckets.map((bucket) => this.client.usage(bucket.since, bucket.until, 'kind', query.source)),
    );
    if (this.range !== range) {
      return;
    }
    const rows: UsageRow[][] = [];
    for (const reply of replies) {
      if (!reply.ok) {
        this.timelineGroup.set_description(GLib.markup_escape_text(reply.error, -1));
        this.timelineLoaded = undefined;
        return;
      }
      rows.push(reply.value);
    }
    const timeline = buildTimeline(buckets, rows);
    this.timelineGroup.set_description('');
    this.chart.setTimeline(timeline);
    this.axisStart.set_label(axisStart(query.since, query.until));
    this.axisEnd.set_label('Now');
    const previous = this.legendSlot.get_first_child();
    if (previous !== null) {
      this.legendSlot.remove(previous);
    }
    this.legendSlot.append(legend(timeline));
  }

  private async loadUsage(summary: Summary, now: number): Promise<void> {
    const query = rangeQuery(this.range, now, summary.sinceUnplug);
    if (query === undefined) {
      this.usageGroup.set_description('The charger hasn’t been unplugged since drainscope started.');
      this.usageRows.set([]);
      return;
    }
    const groupBy = this.byKind ? 'kind' : 'consumer';
    // The current rows stay until the replies arrive, then update in place.
    const [usage, coverage, focus] = await Promise.all([
      this.client.usage(query.since, query.until, groupBy, query.source),
      this.client.coverage(query.since, query.until, query.source),
      this.client.focus(query.since, query.until, query.source),
    ]);
    if (!usage.ok) {
      this.usageGroup.set_description(GLib.markup_escape_text(usage.error, -1));
      this.usageRows.set([]);
      return;
    }
    const span = query.until - query.since;
    // Daemons older than GetCoverage: average over the whole span.
    const measured = coverage.ok ? coverage.value : span;
    // Daemons older than GetFocus: no foreground/background split.
    const model = buildUsage(usage.value, span, measured, this.byKind, appName, focus.ok ? focus.value : []);
    this.usageGroup.set_description(model.entries.length === 0 ? 'Nothing measured in this range yet.' : model.footer);
    this.usageRows.set(model.entries);
  }

  private usageRow(entry: UsageEntry): BoundRow<UsageEntry> {
    let current = entry;
    const row = dataRow(entry.label, '', true);
    row.connect('activated', () => {
      const rangeLabel = RANGES.find((r) => r.id === this.range)?.label ?? '';
      this.navigation.push(detailPage(current, rangeLabel));
    });
    row.add_prefix(new Gtk.Image({ gicon: consumerIcon(entry.consumer), pixel_size: 32 }));
    const share = valueBar();
    row.add_suffix(share.box);
    row.add_suffix(new Gtk.Image({ icon_name: 'go-next-symbolic' }));
    return {
      row,
      update: (e) => {
        current = e;
        const background = e.focus?.mostlyBackground === true ? ' · mostly in the background' : '';
        row.set_title(e.label);
        row.set_subtitle(`${e.energy} · ${e.average}${background}`);
        share.label.set_label(e.share);
        share.bar.set_fraction(e.fraction);
      },
    };
  }

  private async loadWakeups(): Promise<void> {
    const wakeups = await this.client.wakeups();
    // Hidden without the probe, or with a daemon too old to ask.
    const entries = wakeups.ok ? buildWakeups(wakeups.value, WAKEUP_ROWS, appName) : undefined;
    this.wakeupsGroup.set_visible(entries !== undefined && entries.length > 0);
    this.wakeupRows.set(entries ?? []);
  }

  private wakeupRow(entry: WakeupEntry): BoundRow<WakeupEntry> {
    const row = dataRow(entry.label);
    row.add_prefix(new Gtk.Image({ gicon: consumerIcon(entry.consumer), pixel_size: 32 }));
    const rate = valueBar();
    row.add_suffix(rate.box);
    return {
      row,
      update: (e) => {
        row.set_title(e.label);
        rate.label.set_label(e.rate);
        rate.bar.set_fraction(e.fraction);
      },
    };
  }

  private async loadNetwork(): Promise<void> {
    const network = await this.client.network();
    // Hidden without the probe's network counting, or with a daemon too old to ask.
    const entries = network.ok ? buildNetwork(network.value, WAKEUP_ROWS, appName) : undefined;
    this.networkGroup.set_visible(entries !== undefined && entries.length > 0);
    this.networkRows.set(entries ?? []);
  }

  private networkRow(entry: NetworkEntry): BoundRow<NetworkEntry> {
    const row = dataRow(entry.label, entry.traffic);
    row.add_prefix(new Gtk.Image({ gicon: consumerIcon(entry.consumer), pixel_size: 32 }));
    const bar = new Gtk.ProgressBar({ width_request: 80, valign: Gtk.Align.CENTER });
    row.add_suffix(bar);
    return {
      row,
      update: (e) => {
        row.set_title(e.label);
        row.set_subtitle(e.traffic);
        bar.set_fraction(e.fraction);
      },
    };
  }

  private async loadHealth(now: number): Promise<void> {
    if (this.healthLoadedAt !== undefined && now - this.healthLoadedAt < HEALTH_REFRESH_SECONDS) {
      return;
    }
    this.healthLoadedAt = now;
    const readings = await this.client.batteryHealth(now - HEALTH_HISTORY_SECONDS);
    if (!readings.ok) {
      this.healthGroup.set_description(GLib.markup_escape_text(readings.error, -1));
      this.healthRows.set([]);
      this.healthLoadedAt = undefined;
      return;
    }
    const entries = buildHealth(readings.value);
    this.healthGroup.set_description(
      entries.length === 0 ? 'Recorded once a day; the first reading appears within minutes.' : '',
    );
    this.healthRows.set(entries);
  }

  private healthRow(): BoundRow<HealthEntry> {
    const row = dataRow('');
    row.add_prefix(new Gtk.Image({ icon_name: 'battery-level-100-symbolic' }));
    const bar = new Gtk.ProgressBar({ width_request: 80, valign: Gtk.Align.CENTER });
    row.add_suffix(bar);
    return {
      row,
      update: (e) => {
        row.set_title(`${e.battery}: ${e.title}`);
        row.set_subtitle(e.subtitle);
        bar.set_visible(e.fraction !== undefined);
        bar.set_fraction(Math.min(1, e.fraction ?? 0));
      },
    };
  }

  private async loadSleep(now: number): Promise<void> {
    const sessions = await this.client.sleepSessions(now - SLEEP_HISTORY_SECONDS);
    if (!sessions.ok) {
      this.sleepGroup.set_description(GLib.markup_escape_text(sessions.error, -1));
      this.sleepRows.set([]);
      return;
    }
    const entries = buildSleep(sessions.value, now);
    this.sleepGroup.set_description(entries.length === 0 ? 'No suspends in the last 30 days.' : '');
    this.sleepRows.set(entries);
  }
}
