import { HistoryExport } from './HistoryTransfer';
import { useEffect, useMemo, useState } from 'react';
import { CopyDamage } from './CopyDamage';
import { Timeline } from './Timeline';
import { LoadoutProgression, LoadoutTable } from './DungeonLoadout';
import { Icon } from './Icon';
import { Section, ShareBar, StatCard, StatusPill } from './ui';
import { inRecord, type RecordWindow } from './records';
import { compact, duration, precise, type Metric, type DungeonRun, type DungeonSummary, type GameCatalog, type Snapshot } from './types';

export function DungeonJournal({ base, runId, floor, navigate, scope }: { base: string; runId?: string; floor: number; navigate: (path: string) => void; scope?: RecordWindow }) {
  const [runs, setRuns] = useState<DungeonSummary[]>([]);
  const selected = runId ?? '';
  const [run, setRun] = useState<DungeonRun | null>(null);
  const [copied, setCopied] = useState(false);
  const runPath = (id: string, number = 0) => `${base}/${encodeURIComponent(id)}${number ? `/floor/${number}` : ''}`;
  const setSelected = (id: string) => navigate(runPath(id));
  const setFloor = (number: number) => { if (run) navigate(runPath(run.id, number)); };
  const open = () => { if (run) navigate(`${runPath(run.id, floor)}/combat`); };
  async function copyLink() {
    if (!run) return;
    try { await navigator.clipboard.writeText(`${window.location.origin}${window.location.pathname}#${runPath(run.id, floor)}`); setCopied(true); setTimeout(() => setCopied(false), 1500); } catch { setCopied(false); }
  }
  const [metric, setMetric] = useState<Metric>('damage');
  const [resolution, setResolution] = useState(1);
  const [range, setRange] = useState(120);
  const [grouping, setGrouping] = useState<'player' | 'item'>('player');
  const [error, setError] = useState('');
  useEffect(() => {
    let disposed = false;
    let pending = false;
    async function refresh() {
      if (pending) return;
      pending = true;
      try {
        const response = await fetch('/api/dungeons');
        if (!response.ok) throw new Error('Could not load dungeons.');
        const all: DungeonSummary[] = await response.json();
        const items = scope ? all.filter(item => inRecord(item, scope)) : all;
        const matches = selected ? all.filter(item => item.id === selected || item.id.startsWith(selected)) : [];
        const id = selected ? matches.length === 1 ? matches[0].id : selected : items[0]?.id;
        const detail = id ? await fetch(`/api/dungeons/${encodeURIComponent(id)}`) : null;
        const data = detail?.ok ? await detail.json() : null;
        if (!disposed) { setRuns(items); setRun(data); setError(detail && !detail.ok ? `Dungeon ${selected} was not found. Choose a run from the list.` : ''); }
      } catch (failure) { if (!disposed) setError(String(failure)); }
      finally { pending = false; }
    }
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, [selected, scope?.start, scope?.end, scope?.runId]);
  const currentFloor = run?.floors.find(item => item.number === floor);
  const totals = floor === 0 ? run?.totals : currentFloor?.totals;
  const rate = (snapshot: Snapshot, value: Metric) => snapshot.elapsedSeconds > 0 ? (value === 'damage' ? snapshot.totalDamage : snapshot.totalHealing) / snapshot.elapsedSeconds : 0;
  const share = (value: number, denominator: number) => denominator > 0 ? value / denominator * 100 : 0;
  const metricTotal = (snapshot: Snapshot) => metric === 'damage' ? snapshot.totalDamage : snapshot.totalHealing;
  const grouped = totals?.itemContributions?.length ? grouping : 'player';
  const everyone = useMemo(() => new Set(totals?.members.map(member => member.id) ?? []), [totals]);
  return <div className="journal-layout">
    <aside className="panel run-list">
      <div className="panel-heading"><div className="heading-inline"><h2>Runs</h2><span className="count-badge">{runs.length}</span></div></div>
      {error && <p className="run-list-note" role="alert">{error}</p>}
      {runs.length === 0 && <p className="run-list-note">{scope ? 'No dungeons were entered during this record.' : 'No recorded dungeons yet. Entering a dungeon during capture starts a run.'}</p>}
      <div className="run-items">{runs.map(item => <button key={item.id} className={`journal-entry ${run?.id === item.id ? 'active' : ''}`} onClick={() => setSelected(item.id)}>
        <span className="journal-entry-top"><strong>{item.entranceMapName}</strong><StatusPill status={item.status} /></span>
        <span>{item.name} · {item.floors} {item.floors === 1 ? 'floor' : 'floors'} · {duration(item.duration)}</span>
        <span className="journal-entry-values"><b>{compact(item.damage)}</b> dmg <b>{compact(item.healing)}</b> heal</span>
        <small>{new Date(item.startedAt).toLocaleString()}</small>
      </button>)}</div>
    </aside>
    <div className="journal-main">
      {!run && <section className="panel large-empty"><Icon name="layers" size={22} /><h2>{scope ? 'No dungeons in this record' : 'Waiting for dungeon'}</h2><p>Observed dungeon entries and floor changes appear here during capture.</p></section>}
      {run && <>
        <section className="panel run-hero">
          <div className="run-hero-top">
            <div><span className="eyebrow">{run.name}</span><h2>{run.entranceMapName}</h2><p><StatusPill status={run.status} /> Started {new Date(run.startedAt).toLocaleString()} · {run.floors.length} {run.floors.length === 1 ? 'floor' : 'floors'} · {run.mode}</p></div>
            <div className="heading-actions"><HistoryExport dungeonId={run.id} label="Export dungeon" /><button className="button secondary" onClick={() => void copyLink()} title="Copy a link to this run">{copied ? 'Link copied' : 'Copy link'}</button>{totals && <button className="button secondary" onClick={open}>Open combat details</button>}</div>
          </div>
          <div className="floor-tabs" role="tablist" aria-label="Dungeon floor">
            <button role="tab" aria-selected={floor === 0} aria-pressed={floor === 0} onClick={() => setFloor(0)}>All floors<small>{duration(run.totals.elapsedSeconds)}</small></button>
            {run.floors.map(item => <button role="tab" aria-selected={floor === item.number} aria-pressed={floor === item.number} key={item.id} onClick={() => setFloor(item.number)}>Floor {item.number}<small>{duration(item.totals.elapsedSeconds)}</small></button>)}
          </div>
        </section>
        {totals && <>
          <div className="metric-grid">
            <StatCard icon="flame" tone="red" label="Damage" value={compact(totals.totalDamage)} note={floor ? `${share(totals.totalDamage, run.totals.totalDamage).toFixed(1)}% of dungeon` : undefined} />
            <StatCard icon="sword" tone="orange" label="Party DPS" value={precise(rate(totals, 'damage'))} />
            <StatCard icon="heart" tone="green" label="Healing" value={compact(totals.totalHealing)} note={floor ? `${share(totals.totalHealing, run.totals.totalHealing).toFixed(1)}% of dungeon` : undefined} />
            <StatCard icon="zap" tone="blue" label="Party HPS" value={precise(rate(totals, 'healing'))} />
            <StatCard icon="clock" tone="amber" label="Observed time" value={duration(totals.elapsedSeconds)} />
          </div>
          <Section className="chart-panel" title={floor ? `Floor ${floor}` : 'Entire dungeon'} note={currentFloor ? currentFloor.mapName : 'All observed floors combined'} actions={<>
            <div className="segmented" aria-label="Dungeon metric"><button className={metric === 'damage' ? 'active' : ''} aria-pressed={metric === 'damage'} onClick={() => setMetric('damage')}>Damage</button><button className={metric === 'healing' ? 'active' : ''} aria-pressed={metric === 'healing'} onClick={() => setMetric('healing')}>Healing</button></div>
            <select aria-label="Dungeon timeline grouping" value={grouped} onChange={event => setGrouping(event.target.value as 'player' | 'item')}><option value="player">By player</option><option value="item" disabled={!totals.itemContributions?.length}>By item</option></select>
            <select aria-label="Dungeon timeline resolution" value={resolution} onChange={event => setResolution(Number(event.target.value))}>{[1, 5, 10, 30, 60].map(seconds => <option key={seconds} value={seconds}>{seconds === 60 ? '1m' : `${seconds}s`}</option>)}</select>
            <select aria-label="Dungeon timeline range" value={range} onChange={event => setRange(Number(event.target.value))}><option value={30}>Last 30s</option><option value={120}>Last 2m</option><option value={600}>Last 10m</option><option value={0}>Full dungeon</option></select>
            <CopyDamage snapshot={totals} metric={metric} />
          </>}>
            <Timeline snapshot={totals} metric={metric} visible={everyone} range={range} grouping={grouped} resolution={resolution} />
            {grouped === 'player' && <div className="chart-legend">{totals.members.map(member => <span className="legend-entry" key={member.id}><i style={{ background: member.color }} />{member.name}</span>)}</div>}
          </Section>
          <Section title="Players & equipment" count={totals.members.length} note={`${floor ? `Floor ${floor}` : 'Entire dungeon'} · Ranked by ${metric}`}><LoadoutTable snapshot={totals} metric={metric} /></Section>
        </>}
        <Section title="Floors" count={run.floors.length} note={`Share of dungeon ${metric} · Click a floor to inspect it`}><div className="table-scroll"><table><thead><tr><th>FLOOR</th><th>MAP</th><th className="num">TIME</th><th className="num">DAMAGE</th><th className="num">PARTY DPS</th><th className="num">HEALING</th><th className="num">PARTY HPS</th><th>SHARE OF DUNGEON</th></tr></thead><tbody>{run.floors.map(item => <tr key={item.id} className={`clickable ${floor === item.number ? 'selected-row' : ''}`} onClick={() => setFloor(item.number)}><td><strong>Floor {item.number}</strong></td><td><span title={item.mapId}>{item.mapName}</span><small className="history-members">{item.mapId.split('@').at(-1)?.slice(0, 8)}</small></td><td className="num">{duration(item.totals.elapsedSeconds)}</td><td className="num">{precise(item.totals.totalDamage)}</td><td className="num">{precise(rate(item.totals, 'damage'))}</td><td className="num">{precise(item.totals.totalHealing)}</td><td className="num">{precise(rate(item.totals, 'healing'))}</td><td><ShareBar value={share(metricTotal(item.totals), metricTotal(run.totals))} /></td></tr>)}</tbody></table></div></Section>
        <Section title="Progression by floor" note={`${metric === 'damage' ? 'Damage' : 'Healing'} per player and item on each floor`}><LoadoutProgression run={run} metric={metric} /></Section>
        <p className="page-footnote">Rates use observed time on each floor, including idle time. Aggregate timelines join recorded floor segments in time order. Floor numbers follow discovery order.</p>
      </>}
    </div>
  </div>;
}

export function GameLibrary({ tab, onTab: setTab }: { tab: 'weapons' | 'maps'; onTab: (tab: 'weapons' | 'maps') => void }) {
  const [catalog, setCatalog] = useState<GameCatalog | null>(null);
  const [search, setSearch] = useState('');
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  const [storage, setStorage] = useState('');
  useEffect(() => {
    let disposed = false;
    fetch('/api/catalog').then(async response => { if (!response.ok) throw new Error('No map catalog is available.'); const data = await response.json(); if (!disposed) setCatalog(data); }).catch(error => { if (!disposed) setMessage(String(error)); });
    fetch('/api/storage').then(response => response.json()).then(data => { if (!disposed) setStorage(data.databasePath); }).catch(() => { if (!disposed) setStorage('Storage information unavailable.'); });
    return () => { disposed = true; };
  }, []);
  async function importFolder(files: File[]) {
    setBusy(true); setMessage('Reading selected game files…');
    try {
      const worlds = files.filter(file => file.webkitRelativePath.replaceAll('\\', '/').endsWith('/GameData/cluster/world.bin'));
      if (worlds.length !== 1) throw new Error('Select one installed game folder containing GameData/cluster/world.bin.');
      const world = worlds[0];
      const prefix = world.webkitRelativePath.slice(0, -'cluster/world.bin'.length);
      const form = new FormData(); form.append('world', world);
      for (const name of ['items', 'localization']) {
        const file = files.find(item => item.webkitRelativePath === `${prefix}${name}.bin`);
        if (file) form.append(name, file);
      }
      const gameRoot = prefix.split('Albion-Online_Data/')[0];
      const version = files.find(file => file.webkitRelativePath === `${gameRoot}version.txt`);
      if (version && version.size < 1000) form.append('version', (await version.text()).trim());
      const response = await fetch('/api/catalog/import', { method: 'POST', headers: { 'X-Albion-Meter': 'local' }, body: form });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error ?? 'Import failed.');
      setCatalog(result); setMessage(`Imported ${result.mapCount} maps and ${result.weaponCount} weapon definitions.`);
    } catch (error) { setMessage(error instanceof Error ? error.message : 'Import failed.'); }
    finally { setBusy(false); }
  }
  const query = search.toLowerCase();
  const entries = tab === 'weapons' ? catalog?.weapons ?? [] : catalog?.maps ?? [];
  const matches = entries.filter(item => `${item.id} ${item.name} ${'family' in item ? item.family : item.type}`.toLowerCase().includes(query));
  return <div className="library-layout">
    <Section className="catalog-panel" title="Game catalog" note={`Game version ${catalog?.gameVersion ?? 'unknown'}`} actions={<>
      <div className="segmented" aria-label="Catalog"><button className={tab === 'weapons' ? 'active' : ''} aria-pressed={tab === 'weapons'} onClick={() => setTab('weapons')}>Weapons <span>{catalog?.weaponCount ?? 0}</span></button><button className={tab === 'maps' ? 'active' : ''} aria-pressed={tab === 'maps'} onClick={() => setTab('maps')}>Maps <span>{catalog?.mapCount ?? 0}</span></button></div>
      <input className="search wide" placeholder="Search name, family or ID…" aria-label="Search game catalog" value={search} onChange={event => setSearch(event.target.value)} />
    </>}>
      <div className="table-scroll catalog-table"><table><thead><tr><th>NAME</th><th>{tab === 'weapons' ? 'WEAPON FAMILY' : 'MAP TYPE'}</th><th>TIER</th><th>ID</th></tr></thead><tbody>{matches.slice(0, 100).map(item => <tr key={item.id}><td className="strong">{item.name}</td><td>{'family' in item ? item.family : item.type}</td><td>{item.tier ? <span className="subtle-tag">T{item.tier}</span> : <span className="muted">—</span>}</td><td className="mono muted">{item.id}</td></tr>)}</tbody></table>{matches.length === 0 && <div className="empty-list">No {tab} match your search.</div>}</div>
      <div className="table-foot">{precise(matches.length)} results{matches.length > 100 ? ' · Showing the first 100, refine your search for more' : ''}</div>
    </Section>
    <div className="side-stack">
      <Section title="Import game data">
        <div className="side-body">
          <p>Select the installed game folder. Only world.bin, items.bin, localization.bin and the version identifier are read. Processing stays on your local server.</p>
          <label className={`button primary ${busy ? 'disabled' : ''}`}>{busy ? 'Importing…' : 'Import game folder'}<input type="file" multiple disabled={busy} className="folder-input" ref={element => element?.setAttribute('webkitdirectory', '')} onChange={event => { void importFolder(Array.from(event.target.files ?? [])); event.target.value = ''; }} /></label>
          {message && <p className="side-message" role="status">{message}</p>}
        </div>
      </Section>
      <Section title="Catalog">
        <dl className="side-list"><div><dt>Maps</dt><dd>{precise(catalog?.mapCount ?? 0)}</dd></div><div><dt>Weapon definitions</dt><dd>{precise(catalog?.weaponCount ?? 0)}</dd></div><div><dt>Game version</dt><dd>{catalog?.gameVersion ?? 'Unknown'}</dd></div><div><dt>Imported</dt><dd>{catalog?.importedAt ? new Date(catalog.importedAt).toLocaleDateString() : '—'}</dd></div></dl>
        <p className="side-note">Random dungeon instances may only have a generic name in the game dictionary.</p>
      </Section>
      <Section title="Local records">
        <dl className="side-list"><div><dt>Encounters kept</dt><dd>100</dd></div><div><dt>Dungeon runs kept</dt><dd>100</dd></div></dl>
        <p className="side-note storage-path">{storage}</p>
      </Section>
    </div>
  </div>;
}
