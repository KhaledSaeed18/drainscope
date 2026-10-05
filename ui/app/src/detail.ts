import Adw from 'gi://Adw?version=1';
import Gtk from 'gi://Gtk?version=4.0';

import type { UsageEntry } from '@drainscope/shared';

import { consumerIcon } from './apps';
import { dataRow } from './rows';

function shareBar(share: string, fraction: number): Gtk.Widget {
  const box = new Gtk.Box({ orientation: Gtk.Orientation.VERTICAL, valign: Gtk.Align.CENTER, spacing: 4 });
  box.append(new Gtk.Label({ label: share, xalign: 1, css_classes: ['numeric'] }));
  box.append(new Gtk.ProgressBar({ fraction, width_request: 80 }));
  return box;
}

/** Where one consumer's energy went over the selected range (a snapshot, not live). */
export function detailPage(entry: UsageEntry, rangeLabel: string): Adw.NavigationPage {
  const page = new Adw.PreferencesPage();

  const overview = new Adw.PreferencesGroup();
  const header = dataRow(entry.label, rangeLabel);
  header.add_prefix(new Gtk.Image({ gicon: consumerIcon(entry.consumer), pixel_size: 32 }));
  overview.add(header);
  for (const [title, value] of [
    ['Energy', entry.energy],
    ['Average power', entry.average],
    ['Share of the total', entry.share],
  ] as const) {
    const row = dataRow(title);
    row.add_suffix(new Gtk.Label({ label: value, css_classes: ['numeric', 'dim-label'] }));
    overview.add(row);
  }
  page.add(overview);

  const breakdown = new Adw.PreferencesGroup({
    title: 'Breakdown',
    description: 'Where the energy was measured',
  });
  if (entry.parts.length === 0) {
    breakdown.set_description('No energy recorded.');
  }
  for (const part of entry.parts) {
    const row = dataRow(part.label, part.energy);
    row.add_suffix(shareBar(part.share, part.fraction));
    breakdown.add(row);
  }
  page.add(breakdown);

  const toolbar = new Adw.ToolbarView({ content: page });
  toolbar.add_top_bar(new Adw.HeaderBar());
  return new Adw.NavigationPage({ title: entry.label, child: toolbar });
}
