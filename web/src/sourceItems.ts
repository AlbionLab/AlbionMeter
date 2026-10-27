import type { ItemContribution } from './types';

export interface SourceItem extends ItemContribution { contributions: ItemContribution[] }

export function sourceItems(contributions: ItemContribution[]): SourceItem[] {
  const items = new Map<string, SourceItem>();
  for (const contribution of contributions) {
    const id = `${contribution.playerId}:${contribution.slot}:${contribution.itemIndex ?? 'unknown'}${contribution.tierObserved === false ? ':family' : ''}`;
    let item = items.get(id);
    if (!item) {
      item = { ...contribution, id, damage: 0, healing: 0, contributions: [] };
      items.set(id, item);
    }
    item.damage += contribution.damage;
    item.healing += contribution.healing;
    item.contributions.push(contribution);
  }
  return [...items.values()];
}

export function sourceValues(items: SourceItem[], values: Record<string, number> | undefined): Record<string, number> {
  return Object.fromEntries(items.map(item => [item.id, item.contributions.reduce((total, contribution) => total + (values?.[contribution.id] ?? 0), 0)]));
}
