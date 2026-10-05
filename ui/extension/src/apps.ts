import type Gio from 'gi://Gio';
import Shell from 'gi://Shell';

// GNOME Shell's app system already caches desktop entries; looking one up does no I/O.
function lookup(id: string): Shell.App | null {
  return Shell.AppSystem.get_default().lookup_app(`${id}.desktop`);
}

export function appName(id: string): string | undefined {
  return lookup(id)?.get_name() ?? undefined;
}

export function appIcon(id: string): Gio.Icon | null {
  return lookup(id)?.get_icon() ?? null;
}
