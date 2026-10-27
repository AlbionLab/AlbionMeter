export type Weapon = { id: string; name: string; tier: number | null; enchantment: number };
let catalog: Promise<Weapon[]> | null = null;
export function weaponCatalog() {
  if (!catalog) catalog = fetch('/api/catalog').then(response => {
    if (!response.ok) throw new Error();
    return response.json();
  }).then(data => data.indexedWeapons ?? []).catch(() => { catalog = null; return []; });
  return catalog;
}
