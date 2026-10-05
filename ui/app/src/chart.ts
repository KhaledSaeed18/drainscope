import Adw from 'gi://Adw?version=1';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gtk from 'gi://Gtk?version=4.0';

import { TIMELINE_GROUPS, type Timeline, type TimelineBar } from '@drainscope/shared';

type Rgba = readonly [number, number, number, number];

// GNOME palette; apps take the accent color.
const GROUP_COLORS: Readonly<Record<string, Rgba>> = {
  terminals: [0.569, 0.255, 0.675, 1], // purple 3
  services: [0.18, 0.761, 0.494, 1], // green 4
  system: [0.961, 0.761, 0.067, 1], // yellow 4
  idle: [0.6, 0.6, 0.6, 0.45],
};
const BAR_GAP = 3;
const CHART_HEIGHT = 140;

export function groupColor(id: string): Rgba {
  if (id === 'apps') {
    const accent = Adw.StyleManager.get_default().get_accent_color_rgba();
    return [accent.red, accent.green, accent.blue, 1];
  }
  return GROUP_COLORS[id] ?? [0.5, 0.5, 0.5, 1];
}

function timeOf(seconds: number, format: string): string {
  return GLib.DateTime.new_from_unix_local(seconds).format(format) ?? '';
}

/** Where the time axis starts: `09:20`, or `Sun 09:20` when that may be another day. */
export function axisStart(since: number, until: number): string {
  return timeOf(since, until - since > 43_200 ? '%a %H:%M' : '%H:%M');
}

/** `14:00–15:00`, or `Mon 5` for day-long bars. */
export function bucketLabel(bar: TimelineBar): string {
  if (bar.until - bar.since >= 86_400) {
    return timeOf(bar.since, '%a %e').replace(/\s+/g, ' ');
  }
  return `${timeOf(bar.since, '%H:%M')}–${timeOf(bar.until, '%H:%M')}`;
}

/** Stacked bars of energy per time bucket; hover a bar for its time and total. */
export class TimelineChart extends Gtk.DrawingArea {
  static {
    GObject.registerClass(this);
  }

  private timeline: Timeline | undefined;

  constructor() {
    super({ content_height: CHART_HEIGHT, hexpand: true, has_tooltip: true });
    this.set_draw_func((_area, cr, width, height) => {
      const timeline = this.timeline;
      if (timeline === undefined || timeline.bars.length === 0) {
        return;
      }
      const slot = width / timeline.bars.length;
      timeline.bars.forEach((bar, i) => {
        let top = height;
        bar.segments.forEach((joules, group) => {
          const id = TIMELINE_GROUPS[group]?.id ?? '';
          const segment = (joules / timeline.max) * height;
          if (segment <= 0) {
            return;
          }
          const [r, g, b, a] = groupColor(id);
          cr.setSourceRGBA(r, g, b, a);
          cr.rectangle(i * slot + BAR_GAP / 2, top - segment, Math.max(1, slot - BAR_GAP), segment);
          cr.fill();
          top -= segment;
        });
      });
    });
    this.connect('query-tooltip', (_widget, x: number, _y: number, _keyboard: boolean, tooltip: Gtk.Tooltip) => {
      const bars = this.timeline?.bars ?? [];
      const bar = bars[Math.floor((x / this.get_width()) * bars.length)];
      if (bar === undefined) {
        return false;
      }
      tooltip.set_text(`${bucketLabel(bar)} · ${bar.label}`);
      return true;
    });
    Adw.StyleManager.get_default().connect('notify::accent-color', () => {
      this.queue_draw();
    });
  }

  setTimeline(timeline: Timeline): void {
    this.timeline = timeline;
    this.queue_draw();
  }
}

/** Colored keys for the groups in `timeline`. */
export function legend(timeline: Timeline): Gtk.Widget {
  const box = new Gtk.FlowBox({ selection_mode: Gtk.SelectionMode.NONE, column_spacing: 12, max_children_per_line: 5 });
  for (const group of timeline.groups) {
    const key = new Gtk.Box({ spacing: 6 });
    const swatch = new Gtk.DrawingArea({ content_width: 10, content_height: 10, valign: Gtk.Align.CENTER });
    swatch.set_draw_func((_area, cr, width, height) => {
      const [r, g, b, a] = groupColor(group.id);
      cr.setSourceRGBA(r, g, b, a);
      cr.rectangle(0, 0, width, height);
      cr.fill();
    });
    key.append(swatch);
    key.append(new Gtk.Label({ label: group.label, css_classes: ['caption'] }));
    box.append(key);
  }
  return box;
}
