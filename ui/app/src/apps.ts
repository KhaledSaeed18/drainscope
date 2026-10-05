import Gio from 'gi://Gio';
import GioUnix from 'gi://GioUnix';

import { iconName, type Consumer } from '@drainscope/shared';

// GLib 2.86 moved DesktopAppInfo from Gio to GioUnix.
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

/** The app's own icon, or a symbolic icon for the kind of consumer. */
export function consumerIcon(consumer: Consumer | undefined): Gio.Icon {
  const fromApp = consumer?.kind === 'app' ? entry(consumer.name)?.get_icon() : null;
  return fromApp ?? Gio.ThemedIcon.new(consumer === undefined ? 'view-list-symbolic' : iconName(consumer));
}
