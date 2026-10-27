import type { Point } from './types';

export function timelineBuckets(points: Point[], resolution: number): Point[] {
  if (resolution === 1) return points;
  const buckets = new Map<number, Point>();
  const fields = ['damage', 'healing', 'weaponDamage', 'weaponHealing', 'itemDamage', 'itemHealing'] as const;
  for (const point of points) {
    const second = Math.floor(point.second / resolution) * resolution;
    let bucket = buckets.get(second);
    if (!bucket) {
      bucket = { second, damage: {}, healing: {}, weaponDamage: {}, weaponHealing: {}, itemDamage: {}, itemHealing: {} };
      buckets.set(second, bucket);
    }
    for (const field of fields) for (const [id, value] of Object.entries(point[field] ?? {})) bucket[field]![id] = (bucket[field]![id] ?? 0) + value;
  }
  return [...buckets.values()].sort((a, b) => a.second - b.second);
}
