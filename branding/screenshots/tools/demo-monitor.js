// A stand-in for the drainscope daemon that serves illustrative data over Monitor1, for
// screenshots. Run it on a scratch session bus (see capture.sh): gjs -m demo-monitor.js <xml>
//
// Nothing here is measured. The story is a laptop on battery for 2 h 47 min: browsing, a build
// in a terminal, a document, a film, then a video call. Magnitudes follow the dev machine
// (i7-8550U, two 39 Wh batteries now at 31.4 and 30.3 Wh, idle near 5 W from the battery).
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const [xmlPath] = ARGV;
const xml = new TextDecoder().decode(GLib.file_get_contents(xmlPath)[1]);

const T0 = Math.floor(Date.now() / 1000);
const UNPLUG = T0 - (2 * 3600 + 47 * 60);
const CAPACITY_J = (31.39 + 30.33) * 3600;

/** Deterministic noise in [0, 1) per consumer and five-minute slot. */
function noise(key, t) {
  let h = Math.floor(t / 300) * 2654435761;
  for (const c of key) h = Math.imul(h ^ c.charCodeAt(0), 2246822519);
  h ^= h >>> 15;
  return ((h >>> 0) % 1000) / 1000;
}
const vary = (key, t, amount = 0.25) => 1 - amount / 2 + amount * noise(key, t);
const between = (m, a, b) => m >= a && m < b;

/** Local hour of a Unix time, fractional. */
function hourOf(t) {
  const d = GLib.DateTime.new_from_unix_local(t);
  return d.get_hour() + d.get_minute() / 60;
}

/** Local time `days` before today at hh:mm, as Unix seconds. */
function at(days, hh, mm) {
  const today = GLib.DateTime.new_now_local();
  const day = today.add_days(-days);
  return GLib.DateTime.new_local(day.get_year(), day.get_month(), day.get_day_of_month(), hh, mm, 0).to_unix();
}

/** Suspends: [start, end, wake reason], oldest first. */
const SLEEPS = [
  [at(5, 23, 15), at(4, 7, 20), 'Lid (PNP0C0D:00)'],
  [at(3, 12, 30), at(3, 13, 22), 'Power button (PNP0C0C:00)'],
  [at(2, 0, 41), at(2, 7, 21), 'Lid (PNP0C0D:00)'],
  [at(1, 23, 52), at(0, 7, 2), 'Lid (PNP0C0D:00)'],
];

/** No measurements: suspended, or switched off overnight on the other nights. */
function suspended(t) {
  if (t >= UNPLUG) return false;
  if (SLEEPS.some(([start, end]) => t >= start && t < end)) return true;
  const h = hourOf(t);
  const nightOf = Math.floor((t - 7.67 * 3600) / 86400);
  const covered = SLEEPS.some(([start]) => Math.floor((start - 7.67 * 3600) / 86400) === nightOf);
  return !covered && h >= 0.5 && h < 7 + 40 / 60;
}

function source(t) {
  if (t >= UNPLUG) return 'battery';
  const h = hourOf(t);
  return h >= 18 || h < 0.5 ? 'battery' : 'ac';
}

/** Watts as [cpu, gpu, other] per consumer key at time t; t is never suspended. */
function power(t) {
  const p = {
    devices: [0, 0, 2.4],
    idle: [1.25, 0, 0],
    kernel: [0.22, 0, 0],
    shell: [0.28, 0.1, 0],
    'unit:NetworkManager.service': [0.03, 0, 0],
    'app:org.gnome.TextEditor': [0.03, 0, 0],
  };
  const add = (key, cpu, gpu = 0, other = 0) => {
    const k = vary(key, t);
    const was = p[key] ?? [0, 0, 0];
    p[key] = [was[0] + cpu * k, was[1] + gpu * k, was[2] + other];
  };
  if (t >= UNPLUG) {
    // Minutes since unplugging: the story the screenshots show.
    const m = (t - UNPLUG) / 60;
    if (between(m, 0, 15)) add('user-unit:localsearch-3.service', 0.55);
    if (between(m, 0, 10)) add('app:org.gnome.Nautilus', 0.2, 0.04);
    if (between(m, 0, 60)) add('app:org.mozilla.firefox', 1.35, 0.3);
    if (between(m, 60, 125)) add('app:org.mozilla.firefox', 0.12, 0.02);
    if (between(m, 125, 600)) add('app:org.mozilla.firefox', 1.75, 0.75, 0.3);
    if (between(m, 15, 33)) add('term:cargo', 7.0);
    if (between(m, 70, 76)) add('term:cargo', 5.0);
    if (between(m, 15, 33) || between(m, 70, 76)) add('app:org.gnome.Ptyxis', 0.06, 0.02);
    if (between(m, 40, 95)) add('app:libreoffice-writer', 0.24, 0.03);
    if (between(m, 60, 80)) add('app:org.gnome.TextEditor', 0.05);
    if (between(m, 95, 125)) add('app:org.gnome.Showtime', 0.62, 1.25, 0.3);
    if (between(m, 95, 125) || between(m, 125, 600)) add('user-unit:pipewire.service', 0.11);
    if (between(m, 140, 150)) add('app:org.gnome.Software', 0.34, 0.02);
    return p;
  }
  // Earlier days: a working day on the charger, an evening on battery.
  const h = hourOf(t);
  const n = (key) => noise(key, t);
  if (between(h, 8, 18)) {
    add('app:org.mozilla.firefox', 0.9);
    if (n('build') < 0.25) add('term:cargo', 7.5);
    if (between(h, 13, 17)) add('app:libreoffice-writer', 0.22);
    if (n('software') < 0.04) add('app:org.gnome.Software', 0.3);
  } else {
    add('app:org.mozilla.firefox', between(h, 20, 22.5) ? 1.6 : 0.7, 0.4);
    if (between(h, 21, 23)) add('app:org.gnome.Showtime', 0.6, 1.2, 0.3);
    if (between(h, 19, 23)) add('user-unit:pipewire.service', 0.1);
  }
  return p;
}

const STEP = 60;

/** Joules per key over [since, until) for a power-source filter, and measured seconds. */
function integrate(since, until, filter) {
  const totals = new Map();
  let measured = 0;
  for (let t = since; t < until; t += STEP) {
    const dt = Math.min(STEP, until - t);
    const mid = t + dt / 2;
    // Frozen at T0, so every shot of a session shows the same numbers.
    if (mid > T0 || suspended(mid) || (filter !== 'any' && source(mid) !== filter)) continue;
    measured += dt;
    for (const [key, [cpu, gpu, other]] of Object.entries(power(mid))) {
      const was = totals.get(key) ?? [0, 0, 0];
      totals.set(key, [was[0] + cpu * dt, was[1] + gpu * dt, was[2] + other * dt]);
    }
  }
  return { totals, measured };
}

const kindOf = (key) => (key.includes(':') ? key.slice(0, key.indexOf(':')) : key);
const attributable = (key) => !['idle', 'devices', 'platform'].includes(key);

function usageRows(since, until, groupBy, filter) {
  const grouped = new Map();
  for (const [key, split] of integrate(since, until, filter).totals) {
    const kind = kindOf(key);
    const id = groupBy === 'kind' ? kind : key;
    const was = grouped.get(id) ?? { kind, split: [0, 0, 0] };
    grouped.set(id, { kind, split: was.split.map((v, i) => v + split[i]) });
  }
  return [...grouped]
    .map(([key, { kind, split }]) => [key, kind, split[0] + split[1] + split[2], ...split])
    .filter((row) => row[2] > 0)
    .sort((a, b) => b[2] - a[2] || a[0].localeCompare(b[0]));
}

function sleepSessions(since) {
  return SLEEPS.filter(([start]) => start >= since).map(([start, end, reason]) => {
    const wh = 0.055 * ((end - start) / 3600) + 0.02 * noise('sleep', start);
    return [start, end, wh, (wh / (CAPACITY_J / 3600)) * 100, 'deep', reason];
  });
}

function health(since) {
  const readings = [];
  for (let day = 29; day >= 0; day--) {
    const t = T0 - day * 86400;
    if (t < since) continue;
    readings.push(['BAT0', t, 31.39 + day * 0.006, 39.0, 0]);
    readings.push(['BAT1', t, 30.33 + day * 0.01, 39.04, 0]);
  }
  return readings;
}

const service = {
  GetSummary() {
    const rows = usageRows(UNPLUG, Math.floor(Date.now() / 1000), 'consumer', 'battery');
    const total = rows.reduce((s, r) => s + r[2], 0);
    const attributableTotal = rows.filter((r) => attributable(r[0])).reduce((s, r) => s + r[2], 0);
    const top = rows.slice(0, 10).map((r) => [
      r[0],
      r[2],
      (r[2] / CAPACITY_J) * 100,
      attributable(r[0]) ? (r[2] / attributableTotal) * 100 : 0,
    ]);
    return [true, UNPLUG, (total / CAPACITY_J) * 100, top];
  },
  GetUsage(since, until, groupBy, filter) {
    return usageRows(since, until, groupBy, filter);
  },
  GetCoverage(since, until, filter) {
    return integrate(since, until, filter).measured;
  },
  GetSleepSessions(since) {
    return sleepSessions(since).map((s) => s.slice(0, 5));
  },
  GetSleepHistory(since) {
    return sleepSessions(since);
  },
  GetBatteryHealth(since) {
    return health(since);
  },
  GetWakeups() {
    return [true, [
      ['app:org.mozilla.firefox', 38.6],
      ['user-unit:pipewire.service', 24.1],
      ['shell', 16.8],
      ['unit:NetworkManager.service', 5.9],
      ['app:org.gnome.Software', 2.2],
    ]];
  },
  GetNetwork() {
    return [true, [
      ['app:org.mozilla.firefox', 1_620_000, 214_000],
      ['app:org.gnome.Software', 138_000, 9_000],
      ['unit:NetworkManager.service', 410, 190],
    ]];
  },
  get Status() {
    return 'running';
  },
  get ModelVersion() {
    return 2;
  },
  get Domains() {
    return ['package', 'core', 'uncore', 'dram'];
  },
};

const exported = Gio.DBusExportedObject.wrapJSObject(xml, service);
const loop = new GLib.MainLoop(null, false);
Gio.bus_own_name(
  Gio.BusType.SESSION,
  'io.github.khaledsaeed18.Drainscope.Monitor',
  Gio.BusNameOwnerFlags.NONE,
  (connection) => exported.export(connection, '/io/github/khaledsaeed18/Drainscope/Monitor'),
  () => print('demo-monitor: serving illustrative data'),
  () => {
    printerr('demo-monitor: could not own the bus name');
    loop.quit();
  },
);
loop.run();
