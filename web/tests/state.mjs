import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mergeSnapshot } from '../src/state.ts';

const point = (second, damage) => ({ second, damage: { a: damage }, healing: { a: 0 } });
const snapshot = (overrides = {}) => ({
  id: 'one', startedAt: '2026-01-01T00:00:00Z', elapsedSeconds: 2, status: 'running', mode: 'live',
  sequence: 10, eventCount: 2, totalDamage: 15, totalHealing: 0, members: [],
  timeline: [point(0, 5), point(1, 10)], full: true, ...overrides
});
test('Incremental updates retain history and replace provisional buckets', () => {
  const result = mergeSnapshot(snapshot(), snapshot({ sequence: 11, full: false, timeline: [point(1, 15), point(2, 2)] }));
  assert.deepEqual(result.timeline, [point(0, 5), point(1, 15), point(2, 2)]);
});
test('Stale updates cannot overwrite newer totals', () => {
  const previous = snapshot();
  assert.equal(mergeSnapshot(previous, snapshot({ sequence: 9, totalDamage: 1 })), previous);
});
test('Reconnect snapshots replace history without duplicating buckets', () => {
  const next = snapshot({ sequence: 20, timeline: [point(0, 9)] });
  assert.deepEqual(mergeSnapshot(snapshot(), next).timeline, [point(0, 9)]);
});
test('A new encounter clears previous timeline data', () => {
  const next = snapshot({ id: 'two', sequence: 1, startedAt: '2026-01-02T00:00:00Z', timeline: [point(0, 0)] });
  assert.equal(mergeSnapshot(snapshot(), next), next);
});
test('A delayed update from the previous encounter is ignored', () => {
  const current = snapshot({ id: 'two', startedAt: '2026-01-02T00:00:00Z' });
  assert.equal(mergeSnapshot(current, snapshot()), current);
});
test('The initial full snapshot is accepted', () => {
  const next = snapshot();
  assert.equal(mergeSnapshot(null, next), next);
});

const { timelineBuckets } = await import('../src/timelineBuckets.ts');
test('Minute buckets preserve player, weapon and source-item totals at boundaries', () => {
  const points = [59, 60, 119, 120].map(second => ({ second, damage: { a: 10 }, healing: { a: 2 }, weaponDamage: { hammer: 10 }, weaponHealing: { hammer: 2 }, itemDamage: { chest: 10 }, itemHealing: { chest: 2 } }));
  const original = structuredClone(points);
  const minutes = timelineBuckets(points, 60);
  assert.deepEqual(minutes.map(point => [point.second, point.damage.a, point.itemDamage.chest]), [[0, 10, 10], [60, 20, 20], [120, 10, 10]]);
  assert.equal(minutes.reduce((sum, point) => sum + point.itemHealing.chest, 0), 8);
  assert.deepEqual(points, original);
});

const { sourceItems, sourceValues } = await import('../src/sourceItems.ts');
test('Source items combine weapon attacks and abilities while retaining other slots and players', () => {
  const contributions = [
    { id: 'auto', playerId: 'a', slot: 'mainhand', itemIndex: 9022, item: 'Hammer', kind: 'autoAttack', damage: 320, healing: 0 },
    { id: 'skill', playerId: 'a', slot: 'mainhand', itemIndex: 9022, item: 'Hammer', kind: 'ability', damage: 337, healing: 10 },
    { id: 'chest', playerId: 'a', slot: 'chest', itemIndex: 4308, item: 'Jacket', kind: 'ability', damage: 744, healing: 0 },
    { id: 'other', playerId: 'b', slot: 'mainhand', itemIndex: 9022, item: 'Hammer', kind: 'autoAttack', damage: 32, healing: 0 },
    { id: 'unknown', playerId: 'a', slot: 'unknown', itemIndex: null, item: 'Unknown', damage: 5, healing: 0 }
  ];
  const original = structuredClone(contributions);
  const items = sourceItems(contributions);
  assert.equal(items.length, 4);
  const hammer = items.find(item => item.playerId === 'a' && item.itemIndex === 9022);
  assert.equal(hammer.damage, 657);
  assert.equal(hammer.healing, 10);
  assert.equal(hammer.contributions.length, 2);
  const damage = sourceValues(items, { auto: 40, skill: 171, chest: 32, other: 32, unknown: 5 });
  assert.equal(damage[hammer.id], 211);
  assert.equal(Object.values(damage).reduce((sum, value) => sum + value, 0), 280);
  assert.equal(sourceValues(items, { skill: 10 })[hammer.id], 10);
  assert.equal(sourceValues(items, undefined)[hammer.id], 0);
  assert.deepEqual(contributions, original);
});

const { revealTimelineInterval } = await import('../src/timelineNavigation.ts');
test('Moving past either visible edge pans the timeline without clamping the cursor to the window', () => {
  assert.deepEqual(revealTimelineInterval(221, 1, 1000, 120, 100, 220), { second: 221, offset: 778 });
  assert.deepEqual(revealTimelineInterval(99, 1, 1000, 120, 100, 220), { second: 99, offset: 781 });
  assert.deepEqual(revealTimelineInterval(150, 1, 1000, 120, 100, 220), { second: 150, offset: 780 });
});
test('Cursor navigation handles aggregated buckets and stops at actual recording boundaries', () => {
  assert.deepEqual(revealTimelineInterval(1285, 5, 3855, 120, 1162, 1282), { second: 1285, offset: 2565 });
  assert.deepEqual(revealTimelineInterval(-30, 30, 1000, 120, 100, 220), { second: 0, offset: 880 });
  assert.deepEqual(revealTimelineInterval(2000, 10, 1000, 120, 100, 220), { second: 1000, offset: 0 });
  assert.deepEqual(revealTimelineInterval(12, 5, 20, 30, 0, 30), { second: 10, offset: 0 });
});
test('A reset of the same record replaces it even with a lower sequence', () => {
  const reset = snapshot({ sequence: 2, startedAt: '2026-01-01T00:05:00Z', totalDamage: 0, status: 'paused', timeline: [] });
  assert.equal(mergeSnapshot(snapshot(), reset), reset);
});
test('A delayed update from before a Reset of the same record is ignored', () => {
  const reset = snapshot({ sequence: 2, startedAt: '2026-01-01T00:05:00Z', totalDamage: 0, timeline: [] });
  assert.equal(mergeSnapshot(reset, snapshot({ sequence: 50 })), reset);
});
