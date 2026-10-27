import { revealTimelineInterval } from './timelineNavigation';
import { TimelineInspector, type PinnedInterval } from './TimelineInspector';
import { sourceItems, sourceValues } from './sourceItems';
import { Icon } from './Icon';
import { useEffect, useMemo, useRef, useState } from 'react';
import { timelineBuckets } from './timelineBuckets';
import { compact, duration, type Metric, type Point, type Snapshot } from './types';

interface Props { snapshot: Snapshot; metric: Metric; visible: Set<string>; range: number; grouping: 'player' | 'weapon' | 'item'; resolution: number }

type Values = Record<string, number>;
const projections = new WeakMap<Point, Map<string, Values>>();
function projected(point: Point, key: string, compute: () => Values) {
  let cache = projections.get(point);
  if (!cache) { cache = new Map(); projections.set(point, cache); }
  let values = cache.get(key);
  if (!values) { values = compute(); cache.set(key, values); }
  return values;
}
function lowerBound(points: Point[], second: number) {
  let low = 0, high = points.length;
  while (low < high) { const middle = (low + high) >> 1; if (points[middle].second < second) low = middle + 1; else high = middle; }
  return low;
}
const maxFilledSeries = 4;
const maxThickVertices = 3000;

export function Timeline({ snapshot, metric, visible, range, grouping, resolution }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const overlay = useRef<HTMLCanvasElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const plot = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 800, height: 210 });
  const dragging = useRef(false);
  const [offset, setOffset] = useState(0);
  const [pinned, setPinned] = useState<PinnedInterval | null>(null);
  const maxSecond = Math.floor(snapshot.elapsedSeconds);
  const windowSize = range ? Math.min(range, Math.max(30, maxSecond)) : Math.max(30, maxSecond);
  const end = Math.max(windowSize, maxSecond - Math.min(offset, Math.max(0, maxSecond - windowSize)));
  const start = Math.max(0, end - windowSize);
  const latestSecond = Math.floor(Math.min(end, maxSecond) / resolution) * resolution;
  const selection = useMemo<PinnedInterval>(() => {
    if (pinned) return pinned;
    const inside = snapshot.timeline.slice(lowerBound(snapshot.timeline, latestSecond), lowerBound(snapshot.timeline, latestSecond + resolution));
    const point = timelineBuckets(inside, resolution)[0] ?? { second: latestSecond, damage: {}, healing: {} };
    return { snapshot, point, resolution, metric };
  }, [pinned, snapshot, latestSecond, resolution, metric]);
  const weapons = useMemo(() => (snapshot.weaponContributions ?? []).filter(weapon => visible.has(weapon.playerId)), [snapshot.weaponContributions, visible]);
  const items = useMemo(() => sourceItems(snapshot.itemContributions ?? []).filter(item => visible.has(item.playerId)), [snapshot.itemContributions, visible]);
  const players = useMemo(() => {
    const palette = ['#9ebbf3', '#eab978', '#eb95b7', '#67cbd7', '#b69aff', '#6dd1ad', '#f28b82', '#c5d87a'];
    const grouped = grouping === 'item' ? items.map(item => ({ ...item, weapon: item.item })) : weapons;
    return grouping !== 'player' ? grouped.map((weapon, index) => ({ id: weapon.id, name: `${weapon.playerName} · ${weapon.weapon}`, color: palette[index % palette.length] }))
      : [...new Map([...snapshot.members, ...(snapshot.formerMembers ?? [])].map(player => [player.id, player])).values()].filter(player => visible.has(player.id));
  }, [grouping, items, weapons, snapshot.members, snapshot.formerMembers, visible]);
  const hasMetricData = (metric === 'damage' ? snapshot.totalDamage : snapshot.totalHealing) > 0 && snapshot.timeline.length > 0;
  const canInspect = hasMetricData && players.length > 0;
  const itemKey = grouping === 'item' ? items.map(item => `${item.id}=${item.contributions.map(detail => detail.id).join('+')}`).join('|') : '';
  const series = useMemo(() => {
    const key = `${grouping}:${metric}:${itemKey}`;
    const values = (point: Point) => projected(point, key, () => grouping === 'weapon' ? (metric === 'damage' ? point.weaponDamage : point.weaponHealing) ?? {}
      : grouping === 'item' ? sourceValues(items, metric === 'damage' ? point.itemDamage : point.itemHealing) : point[metric]);
    const timeline = snapshot.timeline;
    const seconds: number[] = [];
    const columns = players.map(() => [] as number[]);
    let peak = 0;
    for (let index = lowerBound(timeline, Math.floor(start / resolution) * resolution); index < timeline.length; index++) {
      const point = timeline[index];
      const second = Math.floor(point.second / resolution) * resolution;
      if (second > end) break;
      const own = values(point);
      if (seconds.at(-1) !== second) { seconds.push(second); columns.forEach(column => column.push(0)); }
      const last = seconds.length - 1;
      players.forEach((player, column) => { const value = columns[column][last] += own[player.id] ?? 0; if (value > peak) peak = value; });
    }
    return { seconds, columns, peak };
  }, [snapshot.timeline, resolution, start, end, grouping, metric, itemKey, players, items]);
  function pinInterval(second: number, reveal = false) {
    if (!canInspect) return;
    if (reveal) {
      const next = revealTimelineInterval(second, resolution, maxSecond, windowSize, start, end);
      second = next.second; setOffset(next.offset);
    } else second = Math.max(0, Math.floor(Math.max(start, Math.min(second, end, maxSecond)) / resolution) * resolution);
    const inside = snapshot.timeline.slice(lowerBound(snapshot.timeline, second), lowerBound(snapshot.timeline, second + resolution));
    const point = timelineBuckets(inside, resolution)[0] ?? { second, damage: {}, healing: {} };
    setPinned({ snapshot, point: structuredClone(point), resolution, metric });
  }
  useEffect(() => {
    const observer = new ResizeObserver(entries => {
      const width = Math.round(entries[0].contentRect.width);
      const height = Math.max(100, Math.round(entries[0].contentRect.height));
      setSize(previous => previous.width === width && previous.height === height ? previous : { width, height });
    });
    if (plot.current) observer.observe(plot.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => { setOffset(0); }, [snapshot.id, range]);
  useEffect(() => { setPinned(null); }, [snapshot.id]);
  useEffect(() => { if (pinned) pinInterval(pinned.point.second); }, [range, resolution, offset]);
  useEffect(() => { setPinned(previous => previous ? { ...previous, metric } : null); }, [metric]);
  const left = 48, right = size.width - 16, top = 18, bottom = size.height - 30;
  const maximum = Math.ceil(Math.max(1000, series.peak) / 500) * 500;
  const x = (second: number) => left + (second - start) / windowSize * (right - left);
  const y = (value: number) => bottom - value / maximum * (bottom - top);
  const frame = useRef(0);
  const draw = useRef<() => void>(() => {});
  draw.current = () => {
    const element = canvas.current;
    const context = element?.getContext('2d');
    if (!element || !context) return;
    const { seconds, columns } = series;
    const step = Math.max(1, Math.ceil(seconds.length / Math.max(1, (right - left) / 2)));
    const dense = players.length * Math.ceil(seconds.length / step) * (step > 1 ? 2 : 1) > maxThickVertices;
    const dpr = dense ? 1 : Math.min(window.devicePixelRatio || 1, 2);
    const width = Math.round(size.width * dpr), height = Math.round(size.height * dpr);
    if (element.width !== width || element.height !== height) { element.width = width; element.height = height; }
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.clearRect(0, 0, size.width, size.height);
    context.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    context.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      const value = maximum * i / 4;
      context.strokeStyle = i === 0 ? '#2a323d' : '#161c24';
      context.beginPath(); context.moveTo(left, y(value)); context.lineTo(right, y(value)); context.stroke();
      context.fillStyle = '#66707d'; context.textAlign = 'right'; context.fillText(compact(value), left - 10, y(value) + 3);
    }
    context.textAlign = 'center';
    for (let i = 0; i <= 5; i++) {
      const second = start + windowSize * i / 5;
      context.fillText(duration(second), x(second), size.height - 7);
    }
    const fill = players.length <= maxFilledSeries;
    if (!canInspect) return;
    players.forEach((player, column) => {
      const values = columns[column];
      if (!seconds.length) return;
      const path = new Path2D();
      const visit = (index: number, first: boolean) => { if (first) path.moveTo(x(seconds[index]), y(values[index])); else path.lineTo(x(seconds[index]), y(values[index])); };
      for (let index = 0; index < seconds.length; index += step) {
        if (step === 1) { visit(index, index === 0); continue; }
        let low = index, high = index;
        for (let next = index + 1; next < Math.min(seconds.length, index + step); next++) { if (values[next] < values[low]) low = next; if (values[next] > values[high]) high = next; }
        visit(Math.min(low, high), index === 0); visit(Math.max(low, high), false);
      }
      if (fill) {
        const area = new Path2D(path);
        area.lineTo(x(seconds.at(-1)!), bottom); area.lineTo(x(seconds[0]), bottom); area.closePath();
        const gradient = context.createLinearGradient(0, top, 0, bottom);
        gradient.addColorStop(0, `${player.color}1a`); gradient.addColorStop(1, `${player.color}00`);
        context.fillStyle = gradient; context.fill(area);
      }
      context.strokeStyle = player.color; context.lineWidth = dense ? 1 : 1.5; context.lineJoin = 'round';
      context.stroke(path);
    });
  };
  useEffect(() => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => draw.current());
    return () => cancelAnimationFrame(frame.current);
  }, [series, size, metric, players, canInspect]);
  useEffect(() => {
    const element = overlay.current;
    const context = element?.getContext('2d');
    if (!element || !context) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const width = Math.round(size.width * dpr), height = Math.round(size.height * dpr);
    if (element.width !== width || element.height !== height) { element.width = width; element.height = height; }
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.clearRect(0, 0, size.width, size.height);
    if (!canInspect) return;
    const second = selection.point.second;
    if (second !== undefined && second >= start && second <= end && second <= maxSecond) {
      context.strokeStyle = '#f5a640'; context.lineWidth = 1.5;
      context.beginPath(); context.moveTo(x(second), top); context.lineTo(x(second), bottom); context.stroke();
      context.fillStyle = '#f5a640';
      context.beginPath(); context.moveTo(x(second) - 5, top - 8); context.lineTo(x(second) + 5, top - 8); context.lineTo(x(second), top - 2); context.fill();
      context.font = '11px ui-monospace, monospace'; context.textAlign = 'center';
      context.fillText(duration(second), Math.max(28, Math.min(size.width - 28, x(second))), size.height - 16);
    }
  }, [selection.point.second, size, start, end, maxSecond, canInspect]);
  function moveCursor(key: string, shift: boolean) {
    if (!canInspect) return false;
    if (key === 'Escape') { setPinned(null); return true; }
    if (key === 'ArrowLeft' || key === 'ArrowRight') {
      pinInterval(selection.point.second + (key === 'ArrowLeft' ? -resolution : resolution) * (shift ? 10 : 1), true);
      return true;
    }
    if (key === 'Home' || key === 'End' || key === 'Enter') {
      pinInterval(key === 'End' ? maxSecond : key === 'Home' ? 0 : selection.point.second, true);
      return true;
    }
    return false;
  }
  useEffect(() => {
    const navigate = (event: KeyboardEvent) => {
      if (!(event.target instanceof Element) || event.target.closest('input, select, textarea, button, [contenteditable="true"]')) return;
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
      if (moveCursor(event.key, event.shiftKey)) event.preventDefault();
    };
    window.addEventListener('keydown', navigate);
    return () => window.removeEventListener('keydown', navigate);
  });
  function selectAt(clientX: number, element: HTMLCanvasElement) {
    const rect = element.getBoundingClientRect();
    pinInterval(start + (clientX - rect.left - left) / Math.max(1, right - left) * windowSize);
  }
  return <div className={`timeline-layout${canInspect ? ' has-analysis' : ''}`}>
    <div ref={container} className="timeline">
    <div ref={plot} className="timeline-canvas"><canvas ref={canvas} className={canInspect ? 'timeline-interactive' : ''} style={{ height: size.height }} role={canInspect ? 'slider' : 'img'} aria-label={canInspect ? 'Timeline analysis cursor' : `No ${metric} recorded`} aria-valuemin={canInspect ? 0 : undefined} aria-valuemax={canInspect ? maxSecond : undefined} aria-valuenow={canInspect ? selection.point.second : undefined} aria-valuetext={canInspect ? duration(selection.point.second) : undefined}
      tabIndex={canInspect ? 0 : -1} onPointerDown={event => {
        if (!canInspect || event.button !== 0) return;
        dragging.current = true;
        event.currentTarget.setPointerCapture(event.pointerId);
        event.currentTarget.focus({ preventScroll: true });
        selectAt(event.clientX, event.currentTarget);
      }} onPointerMove={event => {
        if (dragging.current) selectAt(event.clientX, event.currentTarget);
      }} onPointerUp={event => {
        dragging.current = false;
        if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      }} onPointerCancel={() => { dragging.current = false; }} onLostPointerCapture={() => { dragging.current = false; }} onKeyDown={event => {
        if (moveCursor(event.key, event.shiftKey)) { event.preventDefault(); event.stopPropagation(); }
      }} /><canvas ref={overlay} className="timeline-overlay" style={{ height: size.height }} aria-hidden="true" /></div>
    {canInspect && <div className="timeline-cursor-help">Click or drag to inspect an interval · Arrow keys move the cursor</div>}
    {canInspect && grouping !== 'player' && <div className="weapon-series-legend">{players.map(player => <span key={player.id}><i style={{ background: player.color }} />{player.name}</span>)}</div>}
    {!hasMetricData && <div className="chart-empty"><Icon name="chart" size={22} /><strong>{snapshot.eventCount === 0 ? 'Waiting for combat' : `No ${metric} recorded`}</strong><span>{snapshot.mode === 'live' || snapshot.mode === 'replay' ? metric === 'damage' ? 'No attributed damage in this record yet.' : 'No attributed healing in this record yet.' : 'Start a packet capture in Data source to record combat.'}</span></div>}
    {hasMetricData && players.length === 0 && <div className="chart-empty"><strong>Select a player below the chart.</strong></div>}
    {hasMetricData && maxSecond > windowSize && <div className="timeline-scrubber"><span>Earlier</span><input aria-label="Timeline position" type="range" min="0" max={maxSecond - windowSize} value={maxSecond - windowSize - offset} onChange={event => setOffset(maxSecond - windowSize - Number(event.target.value))} /><button onClick={() => setOffset(0)} className={offset === 0 ? 'active-text' : ''}>Latest</button></div>}
    </div>
    {canInspect && <TimelineInspector selection={selection} close={() => setPinned(null)} />}
  </div>;
}
