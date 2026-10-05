import Adw from 'gi://Adw?version=1';
import Gio from 'gi://Gio';
import System from 'system';

import { MonitorClient } from './client';
import { DrainscopeWindow } from './window';

const application = new Adw.Application({
  application_id: 'io.github.khaledsaeed18.Drainscope',
  flags: Gio.ApplicationFlags.DEFAULT_FLAGS,
});
application.connect('activate', () => {
  const existing = application.get_active_window();
  if (existing !== null) {
    existing.present();
    return;
  }
  new DrainscopeWindow(application, new MonitorClient()).present();
});

const status = await application.runAsync([System.programInvocationName, ...ARGV]);
System.exit(status);
