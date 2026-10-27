export type Metric = 'damage' | 'healing';
export interface Player {
  membership?: 'active' | 'left' | 'unconfirmed';
  id: string;
  name: string;
  weapon: string;
  weaponFamily: string;
  color: string;
  damage: number;
  healing: number;
  encounterDps: number;
  encounterHps: number;
  rollingDps: number;
  rollingHps: number;
  damageShare: number;
  healingShare: number;
}
export interface WeaponContribution { id: string; playerId: string; playerName: string; weaponIndex: number | null; weapon: string; color: string; damage: number; healing: number }
export interface ItemContribution { kind?: string; tierObserved?: boolean; id: string; playerId: string; playerName: string; itemIndex: number | null; slot: string; item: string; color: string; damage: number; healing: number }
export interface Point { second: number; damage: Record<string, number>; healing: Record<string, number>; weaponDamage?: Record<string, number>; weaponHealing?: Record<string, number>; itemDamage?: Record<string, number>; itemHealing?: Record<string, number> }
export interface Snapshot {
  id: string;
  startedAt: string;
  elapsedSeconds: number;
  status: 'idle' | 'running' | 'paused' | 'completed';
  mode: string;
  sequence: number;
  eventCount: number;
  totalDamage: number;
  totalHealing: number;
  members: Player[];
  formerMembers?: Player[];
  weaponContributions?: WeaponContribution[];
  itemContributions?: ItemContribution[];
  partyId?: string;
  timeline: Point[];
  full: boolean;
  location?: DungeonLocation;
}
export interface Encounter {
  location?: DungeonLocation;
  id: string;
  startedAt: string;
  duration: number;
  damage: number;
  healing: number;
  members: number;
  mode: string;
}
export const compact = (value: number) => new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(value);
export const precise = (value: number) => new Intl.NumberFormat('en', { maximumFractionDigits: 0 }).format(value);
export function duration(seconds: number) {
  const value = Math.max(0, Math.floor(seconds));
  const pad = (part: number) => part.toString().padStart(2, '0');
  const hours = Math.floor(value / 3600);
  return hours ? `${hours}:${pad(Math.floor(value / 60) % 60)}:${pad(value % 60)}` : `${pad(Math.floor(value / 60))}:${pad(value % 60)}`;
}

export interface DungeonLocation {
  runId: string; name: string; entranceMapId: string; entranceMapName: string;
  floorId: string; floorNumber: number; mapId: string; mapName: string;
}
export interface DungeonFloor {
  id: string; number: number; mapId: string; mapName: string; encounters: Snapshot[]; totals: Snapshot;
}
export interface DungeonRun {
  id: string; startedAt: string; name: string; entranceMapId: string; entranceMapName: string;
  mode: string; status: string; floors: DungeonFloor[]; totals: Snapshot;
}
export interface DungeonSummary {
  id: string; startedAt: string; name: string; entranceMapId: string; entranceMapName: string;
  mode: string; status: string; floors: number; duration: number; damage: number; healing: number;
}
export interface MapDefinition { id: string; name: string; type: string; tier: number | null; file: string; enabled: boolean; exits: string[] }
export interface WeaponDefinition { id: string; name: string; family: string; tier: number | null; slot: string }
export interface GameCatalog {
  schemaVersion: number; source: string; gameVersion: string; importedAt: string; sourceFile: string;
  sha256: string; mapCount: number; maps: MapDefinition[]; weaponCount: number; weapons: WeaponDefinition[];
}

export function sourceKind(kind?: string): string {
  return kind === 'autoAttack' ? 'Auto attack' : kind === 'passive' ? 'Passive' : kind === 'ability' ? 'Ability' : 'Unknown';
}
