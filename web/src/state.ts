import type { Snapshot } from './types';

export function mergeSnapshot(previous: Snapshot | null, next: Snapshot): Snapshot {
  if (previous && previous.id !== next.id && previous.startedAt > next.startedAt) return previous;
  // Reset keeps the record id but starts it again (a new start time and sequence): that snapshot is not an old one.
  if (previous?.id === next.id && previous.startedAt === next.startedAt && previous.sequence > next.sequence) return previous;
  if (next.full || !previous || previous.id !== next.id) return next;
  const points = new Map(previous.timeline.map(point => [point.second, point]));
  next.timeline.forEach(point => points.set(point.second, point));
  return { ...next, full: true, timeline: [...points.values()].sort((a, b) => a.second - b.second) };
}

