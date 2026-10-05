import type Gio from 'gi://Gio';
import GioUnix from 'gi://GioUnix';

// GLib 2.86 moved DesktopAppInfo from Gio to GioUnix.

/** Desktop entries by application id, cached for the session (`null`: none installed). */
const entries = new Map<string, GioUnix.DesktopAppInfo | null>();

function entry(id: string): GioUnix.DesktopAppInfo | null {
  let found = entries.get(id);
  if (found === undefined) {
    found = GioUnix.DesktopAppInfo.new(`${id}.desktop`);
    entries.set(id, found);
  }
  return found;
}

export function appName(id: string): string | undefined {
  return entry(id)?.get_name() ?? undefined;
}

export function appIcon(id: string): Gio.Icon | null {
  return entry(id)?.get_icon() ?? null;
}

/** Forget cached entries, e.g. when the extension is disabled. */
export function clearAppCache(): void {
  entries.clear();
}
