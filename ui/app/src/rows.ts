import Adw from 'gi://Adw?version=1';
import Gtk from 'gi://Gtk?version=4.0';

/**
 * A row showing data as plain text. Titles and subtitles are Pango markup by default, and
 * `use_markup` passed to the constructor is applied after the title is parsed.
 */
export function dataRow(title: string, subtitle = '', activatable = false): Adw.ActionRow {
  // One-line titles: long service names would otherwise wrap over several lines.
  const row = new Adw.ActionRow({ use_markup: false, activatable, title_lines: 1 });
  row.set_title(title);
  row.set_subtitle(subtitle);
  return row;
}

/** A row kept for one key: its widget, and how to show a newer entry in it. */
export interface BoundRow<E> {
  row: Gtk.ListBoxRow;
  update(entry: E): void;
}

/**
 * A boxed list of keyed entries that refreshes in place: a row is created once per key,
 * updated with each newer entry, moved by re-sorting, and removed only when its key goes away.
 * Rebuilding the rows on every refresh would empty the list for a moment, shifting everything
 * below it, and would drop hover and keyboard focus.
 */
export class RowList<E> {
  readonly widget = new Gtk.ListBox({
    css_classes: ['boxed-list'],
    selection_mode: Gtk.SelectionMode.NONE,
    visible: false,
  });
  private readonly rows = new Map<string, BoundRow<E>>();
  private readonly ranks = new Map<Gtk.ListBoxRow, number>();
  private readonly keyOf: (entry: E) => string;
  private readonly create: (entry: E) => BoundRow<E>;

  constructor(keyOf: (entry: E) => string, create: (entry: E) => BoundRow<E>) {
    this.keyOf = keyOf;
    this.create = create;
    this.widget.set_sort_func((a, b) => (this.ranks.get(a) ?? 0) - (this.ranks.get(b) ?? 0));
  }

  /** Shows `entries` in this order; hidden when there are none. */
  set(entries: readonly E[]): void {
    const seen = new Set<string>();
    entries.forEach((entry, rank) => {
      const key = this.keyOf(entry);
      if (seen.has(key)) {
        return;
      }
      seen.add(key);
      let bound = this.rows.get(key);
      if (bound === undefined) {
        bound = this.create(entry);
        this.rows.set(key, bound);
        this.widget.append(bound.row);
      }
      bound.update(entry);
      this.ranks.set(bound.row, rank);
    });
    for (const [key, bound] of this.rows) {
      if (!seen.has(key)) {
        this.widget.remove(bound.row);
        this.ranks.delete(bound.row);
        this.rows.delete(key);
      }
    }
    this.widget.invalidate_sort();
    this.widget.set_visible(seen.size > 0);
  }
}
