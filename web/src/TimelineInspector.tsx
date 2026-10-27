import { useEffect, useMemo, useState } from 'react';
import { ItemIcon, ItemIdentity } from './ItemIcon';
import { sourceItems } from './sourceItems';
import { duration, precise, sourceKind, type Metric, type Point, type Snapshot } from './types';

export interface PinnedInterval { snapshot: Snapshot; point: Point; resolution: number; metric: Metric }

export function TimelineInspector({ selection, close }: { selection: PinnedInterval; close: () => void }) {
  const [metric, setMetric] = useState<Metric>(selection.metric);
  const [scope, setScope] = useState('interval');
  const [order, setOrder] = useState('damage');
  const [search, setSearch] = useState('');
  const { snapshot, point, resolution } = selection;
  useEffect(() => { setMetric(selection.metric); }, [selection.metric]);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') close(); };
    window.addEventListener('keydown', escape);
    return () => window.removeEventListener('keydown', escape);
  }, [close]);
  const allPlayers = useMemo(() => [...new Map([...snapshot.members, ...(snapshot.formerMembers ?? [])].map(player => [player.id, player])).values()], [snapshot]);
  const items = useMemo(() => sourceItems(snapshot.itemContributions ?? []), [snapshot]);
  const value = (player: typeof allPlayers[number], kind: Metric) => scope === 'record' ? player[kind] : point[kind][player.id] ?? 0;
  const total = allPlayers.reduce((sum, player) => sum + value(player, metric), 0);
  const seconds = scope === 'record' ? snapshot.elapsedSeconds : Math.max(0, Math.min(resolution, snapshot.elapsedSeconds - point.second));
  const players = allPlayers.filter(player => player.name.toLowerCase().includes(search.toLowerCase()));
  if (order === 'name') players.sort((a, b) => a.name.localeCompare(b.name));
  if (order === 'damage' || order === 'healing') players.sort((a, b) => value(b, order) - value(a, order) || a.name.localeCompare(b.name));
  return <aside className="timeline-inspector" aria-label="Pinned combat analysis">
    <div className="inspector-heading"><div><strong>Interval analysis</strong><small title={new Date(snapshot.startedAt).toLocaleString()}>{duration(point.second)} – {duration(point.second + resolution)}</small></div><button className="button secondary" onClick={close} aria-label="Follow latest interval" title="Follow latest interval">↦</button></div>
    <div className="inspector-controls"><select aria-label="Analysis scope" value={scope} onChange={event => setScope(event.target.value)}><option value="interval">Selected interval</option><option value="record">Entire record</option></select><select aria-label="Sort analysis" value={order} onChange={event => setOrder(event.target.value)}><option value="damage">Highest damage</option><option value="healing">Highest healing</option><option value="name">Player name</option><option value="party">Party order</option></select><div className="segmented"><button className={metric === 'damage' ? 'active' : ''} onClick={() => setMetric('damage')}>Damage</button><button className={metric === 'healing' ? 'active' : ''} onClick={() => setMetric('healing')}>Healing</button></div><input aria-label="Find player in analysis" placeholder="Find a player…" value={search} onChange={event => setSearch(event.target.value)} /></div>
    <div className="inspector-summary"><span>{precise(total)} {metric}</span><span>{precise(seconds > 0 ? total / seconds : 0)} {metric === 'damage' ? 'DPS' : 'HPS'} · {allPlayers.length} players</span></div>
    <div className="inspector-scroll">{players.map(player => {
      const amount = value(player, metric);
      const contributions = items.filter(item => item.playerId === player.id).map(item => ({ item, value: scope === 'record' ? item[metric] : item.contributions.reduce((sum, detail) => sum + ((metric === 'damage' ? point.itemDamage : point.itemHealing)?.[detail.id] ?? 0), 0) })).filter(item => item.value > 0).sort((a, b) => b.value - a.value);
      const primary = contributions[0]?.item;
      return <details className="inspector-player" key={player.id}>
        <summary className="inspector-compact-row">
          <span className="inspector-avatar" style={{ borderColor: player.color }}><ItemIcon itemIndex={primary?.itemIndex} label={primary?.item ?? player.weapon} tierObserved={primary?.tierObserved} /></span>
          <span className="inspector-player-name"><strong title={player.name}>{player.name}</strong><small><span title="Damage">D {precise(value(player, 'damage'))}</span><span title="Healing">H {precise(value(player, 'healing'))}</span><span>{precise(seconds > 0 ? amount / seconds : 0)} {metric === 'damage' ? 'DPS' : 'HPS'}</span></small></span>
          <span className="inspector-player-value"><b>{precise(amount)}</b><small>{(total ? amount / total * 100 : 0).toFixed(1)}%</small></span>
          <span className="inspector-expand" aria-hidden="true">›</span>
        </summary>
        <div className="inspector-item-details">{contributions.map(({ item, value: itemValue }) => <div className="inspector-item" key={item.id}><ItemIdentity itemIndex={item.itemIndex} label={item.item} tierObserved={item.tierObserved}><span>{item.item}</span><small>{item.contributions.map(detail => ({ detail, amount: scope === 'record' ? detail[metric] : (metric === 'damage' ? point.itemDamage : point.itemHealing)?.[detail.id] ?? 0 })).filter(detail => detail.amount > 0).map(({ detail, amount }) => `${sourceKind(detail.kind)} ${precise(amount)}`).join(' · ')}</small></ItemIdentity><b>{precise(itemValue)}</b></div>)}{contributions.length === 0 && <span className="subtle">No attributed source items in this interval.</span>}</div>
      </details>;
    })}{players.length === 0 && <p className="tooltip-note">No players match this search.</p>}</div>
    <small className="inspector-footnote">← → Move cursor · Expand a player to inspect items</small>
  </aside>;
}
