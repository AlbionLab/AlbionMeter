import { useEffect, useState, type ReactNode } from 'react';
import { weaponCatalog, type Weapon } from './weapons';

const definitions = new Map<number, Promise<Weapon | null>>();
function definition(index: number) {
  let pending = definitions.get(index);
  if (!pending) {
    pending = fetch(`/api/items/${index}`).then(response => response.ok ? response.json() as Promise<Weapon> : null).catch(() => { definitions.delete(index); return null; });
    definitions.set(index, pending);
  }
  return pending;
}

export function ItemIcon({ itemIndex, label, tierObserved = true }: { itemIndex?: number | null; label: string; tierObserved?: boolean }) {
  const key = `${itemIndex ?? ''}:${label}`;
  const [resolved, setResolved] = useState<{ key: string; item: Weapon | null } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    const pending = itemIndex != null ? itemIndex > 0 ? definition(itemIndex) : Promise.resolve(null)
      : weaponCatalog().then(items => items.find(item => `${item.name} · T${item.tier}.${item.enchantment}` === label) ?? null);
    void pending.then(item => { if (!disposed) setResolved({ key, item }); });
    return () => { disposed = true; };
  }, [key, itemIndex, label]);
  const item = resolved?.key === key ? resolved.item : null;
  return <span className="item-icon" title={label}>
    <span className="item-icon-image">{item && failed !== item.id ? <img key={item.id} src={`/api/item-icon/${encodeURIComponent(item.id)}`} alt={label} loading="lazy" onError={() => setFailed(item.id)} /> : <span aria-label="Item image unavailable">?</span>}</span>
    {item?.tier != null && tierObserved && <span className="item-tier">T{item.tier}.{item.enchantment}</span>}
  </span>;
}

export function ItemIdentity({ itemIndex, label, tierObserved, children }: { itemIndex?: number | null; label: string; tierObserved?: boolean; children: ReactNode }) {
  return <span className="item-identity"><ItemIcon itemIndex={itemIndex} label={label} tierObserved={tierObserved} /><span className="item-identity-text">{children}</span></span>;
}
