import { useState } from 'react';
import { ItemIdentity } from './ItemIcon';
import { sourceItems, type SourceItem } from './sourceItems';
import { ShareBar } from './ui';
import { precise, sourceKind, type DungeonRun, type Metric, type Snapshot } from './types';

const slotNames: Record<string, string> = { mainhand: 'Main hand', offhand: 'Off hand', head: 'Head', armor: 'Chest', chest: 'Chest', shoes: 'Shoes', cape: 'Cape', bag: 'Bag', mount: 'Mount', food: 'Food', potion: 'Potion' };
const slotOrder = Object.keys(slotNames);
export const slotName = (slot: string) => slotNames[slot.toLowerCase()] ?? slot;
const percent = (value: number, total: number) => `${(total > 0 ? value / total * 100 : 0).toFixed(1)}%`;
const rate = (value: number, seconds: number) => seconds > 0 ? value / seconds : 0;

function itemsByPlayer(snapshot: Snapshot) {
  const result = new Map<string, SourceItem[]>();
  for (const item of sourceItems(snapshot.itemContributions ?? [])) result.set(item.playerId, [...result.get(item.playerId) ?? [], item]);
  return result;
}
function sorted(items: SourceItem[], metric: Metric) {
  return [...items].sort((a, b) => b[metric] - a[metric] || slotOrder.indexOf(a.slot) - slotOrder.indexOf(b.slot));
}
function sources(item: SourceItem, metric: Metric) {
  return item.contributions.filter(detail => detail[metric] > 0).map(detail => `${sourceKind(detail.kind)} ${precise(detail[metric])}`).join(' · ');
}

function combatWeapon(weapon: string, items: SourceItem[]) {
  if (weapon !== 'Weapon not observed') return null;
  const weapons = items.filter(item => item.slot.toLowerCase() === 'mainhand' && item.itemIndex);
  const main = [...weapons].sort((a, b) => b.damage - a.damage || b.healing - a.healing)[0];
  return main ? { item: main, label: `${main.item}${weapons.length > 1 ? ` +${weapons.length - 1}` : ''} · seen in combat` } : null;
}

function useExpanded(ids: string[]) {
  const [open, setOpen] = useState<Set<string> | null>(null);
  const expanded = open ?? new Set(ids.length <= 5 ? ids : []);
  const toggle = (id: string) => setOpen(() => { const next = new Set(expanded); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  const all = ids.length > 0 && ids.every(id => expanded.has(id));
  const control = ids.length > 1 && <button className="button secondary loadout-toggle" onClick={() => setOpen(all ? new Set() : new Set(ids))}>{all ? 'Collapse all' : 'Expand all'}</button>;
  return { expanded, toggle, control };
}

export function LoadoutTable({ snapshot, metric }: { snapshot: Snapshot; metric: Metric }) {
  const items = itemsByPlayer(snapshot);
  const seconds = snapshot.elapsedSeconds;
  const { expanded, toggle, control } = useExpanded(snapshot.members.map(member => member.id));
  return <>{control}<div className="table-scroll"><table className="loadout-table">
    <thead><tr><th>PLAYER / ITEM</th><th>SLOT</th><th className="num">DAMAGE</th><th>SHARE</th><th className="num">DPS</th><th className="num">HEALING</th><th>SHARE</th><th className="num">HPS</th></tr></thead>
    {[...snapshot.members].sort((a, b) => b[metric] - a[metric]).map(member => {
      const own = sorted(items.get(member.id) ?? [], metric);
      const seen = combatWeapon(member.weapon, own);
      return <tbody key={member.id} className="loadout-group">
        <tr className={`loadout-player ${expanded.has(member.id) ? 'open' : ''}`} onClick={() => toggle(member.id)} aria-expanded={expanded.has(member.id)}><td><ItemIdentity itemIndex={seen?.item.itemIndex} label={seen?.item.item ?? member.weapon} tierObserved={seen?.item.tierObserved}><span className="player-dot" style={{ background: member.color }} /><strong>{member.name}</strong><small className="history-members">{seen?.label ?? member.weapon} · {own.length} {own.length === 1 ? 'source' : 'sources'}</small></ItemIdentity></td><td />
          <td className="num">{precise(member.damage)}</td><td><ShareBar value={member.damageShare} color={member.color} /></td><td className="num">{precise(member.encounterDps)}</td>
          <td className="num">{precise(member.healing)}</td><td><ShareBar value={member.healingShare} color={member.color} /></td><td className="num">{precise(member.encounterHps)}</td></tr>
        {expanded.has(member.id) && own.map(item => <tr key={item.id} className="loadout-item"><td><ItemIdentity itemIndex={item.itemIndex} label={item.item} tierObserved={item.tierObserved}><span>{item.item}</span><small className="history-members">{sources(item, metric)}</small></ItemIdentity></td><td className="muted">{slotName(item.slot)}</td>
          <td className="num">{item.damage ? precise(item.damage) : '—'}</td><td>{item.damage > 0 && <ShareBar value={member.damage > 0 ? item.damage / member.damage * 100 : 0} color={`${member.color}88`} />}</td><td className="num">{item.damage ? precise(rate(item.damage, seconds)) : ''}</td>
          <td className="num">{item.healing ? precise(item.healing) : '—'}</td><td>{item.healing > 0 && <ShareBar value={member.healing > 0 ? item.healing / member.healing * 100 : 0} color={`${member.color}88`} />}</td><td className="num">{item.healing ? precise(rate(item.healing, seconds)) : ''}</td></tr>)}
        {expanded.has(member.id) && own.length === 0 && <tr className="loadout-item"><td colSpan={8} className="muted">No item sources observed</td></tr>}
      </tbody>;
    })}
  </table></div></>;
}

export function LoadoutProgression({ run, metric }: { run: DungeonRun; metric: Metric }) {
  const floors = run.floors.map(floor => ({ floor, items: new Map(sourceItems(floor.totals.itemContributions ?? []).map(item => [item.id, item])) }));
  const items = itemsByPlayer(run.totals);
  const total = metric === 'damage' ? run.totals.totalDamage : run.totals.totalHealing;
  const unit = metric === 'damage' ? 'DPS' : 'HPS';
  const { expanded, toggle, control } = useExpanded(run.totals.members.map(member => member.id));
  return <>{control}<div className="table-scroll"><table className="loadout-table">
    <thead><tr><th>PLAYER / ITEM</th>{run.floors.map(floor => <th className="num" key={floor.id}>FLOOR {floor.number}</th>)}<th className="num">DUNGEON TOTAL</th></tr></thead>
    {[...run.totals.members].sort((a, b) => b[metric] - a[metric]).map(member => { const seen = combatWeapon(member.weapon, items.get(member.id) ?? []); return <tbody key={member.id} className="loadout-group">
      <tr className={`loadout-player ${expanded.has(member.id) ? 'open' : ''}`} onClick={() => toggle(member.id)} aria-expanded={expanded.has(member.id)}><td><ItemIdentity itemIndex={seen?.item.itemIndex} label={seen?.item.item ?? member.weapon} tierObserved={seen?.item.tierObserved}><strong>{member.name}</strong><small className="history-members">{seen?.label ?? member.weapon}</small></ItemIdentity></td>{floors.map(({ floor }) => {
        const value = floor.totals.members.find(player => player.id === member.id);
        return <td className="num" key={floor.id}>{precise(value?.[metric] ?? 0)}<small className="history-members">{precise(value ? metric === 'damage' ? value.encounterDps : value.encounterHps : 0)} {unit} · {percent(value?.[metric] ?? 0, metric === 'damage' ? floor.totals.totalDamage : floor.totals.totalHealing)} PT</small></td>;
      })}<td className="num">{precise(member[metric])}<small className="history-members">{percent(member[metric], total)} PT</small></td></tr>
      {expanded.has(member.id) && sorted(items.get(member.id) ?? [], metric).filter(item => item[metric] > 0).map(item => <tr key={item.id} className="loadout-item"><td><ItemIdentity itemIndex={item.itemIndex} label={item.item} tierObserved={item.tierObserved}><span>{item.item}</span><small className="history-members">{slotName(item.slot)}</small></ItemIdentity></td>{floors.map(({ floor, items: floorItems }) => {
        const value = floorItems.get(item.id)?.[metric] ?? 0;
        const player = floor.totals.members.find(entry => entry.id === member.id)?.[metric] ?? 0;
        return <td className="num" key={floor.id}>{value ? precise(value) : '—'}{value > 0 && <small className="history-members">{precise(rate(value, floor.totals.elapsedSeconds))} {unit} · {percent(value, player)}</small>}</td>;
      })}<td className="num">{precise(item[metric])}<small className="history-members">{percent(item[metric], member[metric])} of player</small></td></tr>)}
    </tbody>; })}
  </table></div></>;
}
