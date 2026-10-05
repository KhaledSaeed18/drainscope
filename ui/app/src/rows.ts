import Adw from 'gi://Adw?version=1';

/**
 * A row showing data as plain text. Titles and subtitles are Pango markup by default, and
 * `use_markup` passed to the constructor is applied after the title is parsed.
 */
export function dataRow(title: string, subtitle = '', activatable = false): Adw.ActionRow {
  const row = new Adw.ActionRow({ use_markup: false, activatable });
  row.set_title(title);
  row.set_subtitle(subtitle);
  return row;
}
