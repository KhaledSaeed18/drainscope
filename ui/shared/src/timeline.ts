/** Stacked energy-over-time bars for the desktop app. */

import type { UsageRow } from './monitor';
import { formatEnergy } from './units';
import type { Range } from './views';

export interface Bucket {
  /** Unix seconds, `[since, until)`. */
  since: number;
  until: number;
}

export interface TimelineGroup {
  id: string;
  label: string;
  kinds: readonly string[];
}

/** Bottom to top. Kinds not listed count as system. */
export const TIMELINE_GROUPS: readonly TimelineGroup[] = [
  { id: 'apps', label: 'Apps', kinds: ['app'] },
  { id: 'terminals', label: 'Terminals', kinds: ['term'] },
  {
    id: 'services',
    label: 'Services & sessions',
    kinds: ['unit', 'user-unit', 'container', 'session', 'shell', 'other-users', 'root', 'drainscope'],
  },
  { id: 'system', label: 'System & devices', kinds: ['devices', 'platform', 'kernel', 'exited'] },
  { id: 'idle', label: 'Idle', kinds: ['idle'] },
];

const BUCKETS: Readonly<Record<Range, { count: number; seconds?: number }>> = {
  hour: { count: 12, seconds: 300 },
  day: { count: 24, seconds: 3600 },
  week: { count: 7, seconds: 86_400 },
  // Since unplugged: the span split evenly, at least 5 minutes per bar.
  unplug: { count: 12 },
};
const MIN_BUCKET_SECONDS = 300;

/** Buckets covering `[since, until)`, oldest first, ending at `until`. */
export function timelineBuckets(range: Range, since: number, until: number): Bucket[] {
  const plan = BUCKETS[range];
  const span = until - since;
  const width = plan.seconds ?? Math.max(MIN_BUCKET_SECONDS, Math.ceil(span / plan.count));
  const count = plan.seconds === undefined ? Math.max(1, Math.ceil(span / width)) : plan.count;
  const buckets: Bucket[] = [];
  for (let i = count; i > 0; i--) {
    buckets.push({ since: Math.max(since, until - i * width), until: until - (i - 1) * width });
  }
  return buckets;
}

export interface TimelineBar extends Bucket {
  /** Joules per group, in `TIMELINE_GROUPS` order. */
  segments: number[];
  total: number;
  /** e.g. `0.42 Wh`. */
  label: string;
}

export interface Timeline {
  bars: TimelineBar[];
  /** The tallest bar's joules (at least 1, so empty timelines scale). */
  max: number;
  /** Groups with any energy, for the legend. */
  groups: TimelineGroup[];
}

function groupIndex(kind: string): number {
  const index = TIMELINE_GROUPS.findIndex((group) => group.kinds.includes(kind));
  return index === -1 ? TIMELINE_GROUPS.findIndex((group) => group.id === 'system') : index;
}

/** `rows[i]` is `GetUsage` grouped by kind over `buckets[i]`. */
export function buildTimeline(buckets: readonly Bucket[], rows: readonly (readonly UsageRow[])[]): Timeline {
  const used = new Set<number>();
  const bars = buckets.map((bucket, i) => {
    const segments: number[] = TIMELINE_GROUPS.map(() => 0);
    for (const row of rows[i] ?? []) {
      const index = groupIndex(row.key);
      segments[index] = (segments[index] ?? 0) + row.total;
      if (row.total > 0) {
        used.add(index);
      }
    }
    const total = segments.reduce((sum, joules) => sum + joules, 0);
    return { ...bucket, segments, total, label: formatEnergy(total) };
  });
  const max = Math.max(1, ...bars.map((bar) => bar.total));
  return { bars, max, groups: TIMELINE_GROUPS.filter((_, i) => used.has(i)) };
}
