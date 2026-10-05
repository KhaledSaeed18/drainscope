import { describe, expect, it } from 'vitest';

import { buildTimeline, timelineBuckets, TIMELINE_GROUPS } from './timeline';

const row = (key: string, total: number) => ({ key, kind: key, total, cpu: total, gpu: 0, other: 0 });

describe('timelineBuckets', () => {
  it('uses fixed widths for fixed ranges', () => {
    const buckets = timelineBuckets('day', 0, 86_400);
    expect(buckets).toHaveLength(24);
    expect(buckets[0]).toEqual({ since: 0, until: 3600 });
    expect(buckets[23]).toEqual({ since: 82_800, until: 86_400 });
  });

  it('splits the time since unplugging, at least 5 minutes per bar', () => {
    expect(timelineBuckets('unplug', 1000, 1000 + 3600)).toHaveLength(12);
    const short = timelineBuckets('unplug', 1000, 1000 + 700);
    expect(short).toEqual([
      { since: 1000, until: 1100 },
      { since: 1100, until: 1400 },
      { since: 1400, until: 1700 },
    ]);
  });
});

describe('buildTimeline', () => {
  it('stacks kinds into groups', () => {
    const buckets = [
      { since: 0, until: 10 },
      { since: 10, until: 20 },
    ];
    const timeline = buildTimeline(buckets, [
      [row('app', 3600), row('user-unit', 360), row('idle', 1800), row('mystery', 36)],
      [],
    ]);
    const ids = TIMELINE_GROUPS.map((g) => g.id);
    expect(ids).toEqual(['apps', 'terminals', 'services', 'system', 'idle']);
    expect(timeline.bars[0]?.segments).toEqual([3600, 0, 360, 36, 1800]);
    expect(timeline.bars[0]?.label).toBe('1.61 Wh');
    expect(timeline.bars[1]?.total).toBe(0);
    expect(timeline.max).toBe(5796);
    expect(timeline.groups.map((g) => g.id)).toEqual(['apps', 'services', 'system', 'idle']);
  });

  it('scales empty timelines', () => {
    expect(buildTimeline([{ since: 0, until: 1 }], [[]]).max).toBe(1);
  });
});
