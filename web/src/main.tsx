import { createRoot } from 'react-dom/client';
import { useEffect, useState, type CSSProperties } from 'react';
import { useMeter } from './useMeter';
import { CopyDamage } from './CopyDamage';
import { CapturePanel } from './CapturePanel';
import { CaptureNotice } from './CaptureNotice';
import { AppLoading, UpdateToast, WindowControls } from './DesktopChrome';
import { desktop } from './desktop';
import { GameLibrary } from './Library';
import { Icon } from './Icon';
import { Select } from './Select';
import { weaponCatalog } from './weapons';
import { compact, duration, precise, type Metric, type Player } from './types';
import './styles.css';

type Status = {
  server: string; connected: boolean; account: { name: string; avatarUrl: string } | null;
  pairing: { code: string; verifyUrl: string; expiresAt: string } | null; pairingError: string | null;
  session: { id: string; url: string; visibility: string; anonymize: boolean; state: string; error: string | null; publishedAt: string | null; title: string | null } | null;
  shareLocation: boolean; visibility: string; anonymize: boolean;
};

const post = async <T,>(path: string, body?: unknown): Promise<T> => {
  const response = await fetch(path, { method: 'POST', headers: { 'X-Albion-Meter': 'local', ...(body ? { 'Content-Type': 'application/json' } : {}) }, body: body ? JSON.stringify(body) : undefined });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error ?? 'Request failed.');
  return value as T;
};

function WeaponIcon({ player }: { player: Player }) {
  const [id, setId] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    void weaponCatalog().then(weapons => {
      const weapon = weapons.find(item => `${item.name} · T${item.tier}.${item.enchantment}` === player.weapon);
      if (!disposed) setId(weapon?.id ?? null);
    });
    return () => { disposed = true; };
  }, [player.weapon]);
  return <span className="bar-icon" title={player.weapon}>{id ? <img src={`/api/weapon-icon/${encodeURIComponent(id)}`} alt="" /> : <span>?</span>}</span>;
}

function SessionName({ title, save }: { title: string | null; save: (title: string) => void }) {
  const [value, setValue] = useState(title ?? '');
  useEffect(() => setValue(title ?? ''), [title]);
  const commit = () => { if (value.trim() !== (title ?? '')) save(value.trim()); };
  return <label className="albionlab-field albionlab-name"><span>Name</span><input value={value} maxLength={80} placeholder="Untitled session" aria-label="Session name"
    onChange={event => setValue(event.target.value)} onBlur={commit} onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') { setValue(title ?? ''); event.currentTarget.blur(); } }} /></label>;
}

function AlbionLabCard({ recording }: { recording: boolean }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState('');
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    let disposed = false;
    const load = () => fetch('/api/albionlab').then(response => response.json()).then(value => { if (!disposed) setStatus(value); }).catch(() => undefined);
    void load();
    const timer = setInterval(load, 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, []);
  const run = async (action: () => Promise<Status>) => {
    setError('');
    try { setStatus(await action()); } catch (failure) { setError(failure instanceof Error ? failure.message : 'AlbionLab is unavailable.'); }
  };
  const connect = () => run(async () => {
    const next = await post<Status>('/api/albionlab/connect');
    if (next.pairing) window.open(next.pairing.verifyUrl, '_blank', 'noopener');
    return next;
  });
  if (!status) return <section className="panel albionlab-card"><p className="muted">Connecting to AlbionLab…</p></section>;
  const session = status.session?.id ? status.session : null;
  return <section className="panel albionlab-card">
    <div className="albionlab-head">
      <img src="/brand.svg" alt="AlbionLab" width={34} height={27} />
      <div><strong>AlbionLab</strong><span className="muted">Full analysis, dungeons and sharing at albionlab.app</span></div>
      {status.account && <span className="albionlab-account"><img src={status.account.avatarUrl} alt="" width={26} height={26} />{status.account.name}</span>}
    </div>
    {error && <p className="albionlab-error">{error}</p>}
    {!status.connected && !status.pairing && <div className="albionlab-body">
      <p>Connect your AlbionLab account to publish each record as a live session you can open, share or make public.</p>
      {status.pairingError === 'revoked' && <p className="albionlab-error">This PC was disconnected from your account. Connect again to keep publishing.</p>}
      {status.pairingError === 'denied' && <p className="albionlab-error">The connection was refused on the site.</p>}
      {status.pairingError === 'expired' && <p className="albionlab-error">The code expired. Try again.</p>}
      <button className="button primary" onClick={() => void connect()}><Icon name="link" size={15} />Connect account</button>
    </div>}
    {!status.connected && status.pairing && <div className="albionlab-body">
      <p>Approve this PC on AlbionLab (sign in with Discord if asked). The code shown there must be:</p>
      <div className="pair-code">{status.pairing.code}</div>
      <div className="albionlab-actions"><a className="button secondary" href={status.pairing.verifyUrl} target="_blank" rel="noreferrer">Open AlbionLab again</a><span className="muted">Waiting for approval…</span></div>
    </div>}
    {status.connected && <div className="albionlab-body">
      {session ? <>
        <div className="albionlab-session">
          <span className={`publish-state ${session.state}`}><i />{session.state === 'live' ? 'Publishing live' : session.state === 'completed' ? 'Session finished' : session.state === 'offline' ? 'Offline — will resend' : 'Not published'}</span>
          <a className="session-link" href={session.url} target="_blank" rel="noreferrer">{session.url.replace(/^https?:\/\//, '')}</a>
          <button className="button secondary" onClick={() => { void navigator.clipboard.writeText(session.url); setCopied(true); setTimeout(() => setCopied(false), 1500); }}>{copied ? 'Copied' : 'Copy link'}</button>
          <a className="button primary" href={session.url} target="_blank" rel="noreferrer">Open full analysis</a>
        </div>
        {session.error && <p className="albionlab-error">{session.error}</p>}
        <SessionName key={session.id} title={session.title} save={title => void run(() => post('/api/albionlab/session', { title }))} />
      </> : <div className="albionlab-session">
        <span className="publish-state"><i />{recording ? 'Creating the session…' : 'No live session'}</span>
        <span className="muted">{recording ? 'The live session on AlbionLab is being created.' : 'Press Play: each record becomes a new live session on AlbionLab, and Stop ends it.'}</span>
        <a className="button secondary" href={`${status.server}/account`} target="_blank" rel="noreferrer">My sessions</a>
      </div>}
      <div className="albionlab-options">
        <div className="albionlab-field"><span>Visibility</span><Select aria-label="Visibility" value={session?.visibility ?? status.visibility} onChange={visibility => void run(() => post('/api/albionlab/session', { visibility }))}
          options={[{ value: 'unlisted', label: 'Unlisted — anyone with the link' }, { value: 'public', label: 'Public — listed on AlbionLab' }, { value: 'private', label: 'Private — only me' }]} /></div>
        <label className="check"><input type="checkbox" checked={session?.anonymize ?? status.anonymize} onChange={event => void run(() => post('/api/albionlab/session', { anonymize: event.target.checked }))} />Hide other players' names</label>
      </div>
      {session?.error && !session.id && <p className="albionlab-error">{session.error}</p>}
      <div className="albionlab-options">
        <label className="check"><input type="checkbox" checked={status.shareLocation} onChange={event => void run(() => post('/api/albionlab/location', { shareLocation: event.target.checked }))} />Share dungeon location (map names)</label>
        <button className="link-button" onClick={() => { if (window.confirm('Disconnect this PC from your AlbionLab account?')) void run(() => post('/api/albionlab/disconnect')); }}>Disconnect account</button>
      </div>
    </div>}
  </section>;
}

function Meter() {
  const { snapshot, connection, error, busy, command, dismissError } = useMeter();
  const [metric, setMetric] = useState<Metric>('damage');
  const [view, setView] = useState<'meter' | 'capture' | 'game'>('meter');
  const [catalogTab, setCatalogTab] = useState<'weapons' | 'maps'>('weapons');
  const data = snapshot;
  const live = data?.mode === 'live';
  const status = live ? data.status : 'idle';
  const canControl = connection === 'connected' && !busy;
  const players = [...(data?.members ?? [])].filter(player => player.membership !== 'unconfirmed' || player.damage + player.healing > 0).sort((a, b) => b[metric] - a[metric]);
  const top = players[0]?.[metric] ?? 0;
  const total = data ? metric === 'damage' ? data.totalDamage : data.totalHealing : 0;
  const unit = metric === 'damage' ? 'DPS' : 'HPS';
  return <div className={`mini-shell${desktop ? ' desktop' : ''}`}>
    <header className="mini-bar">
      <span className="brand"><img src="/mark.svg" alt="" /><span>Albion Meter</span></span>
      <nav className="segmented" aria-label="View">
        <button className={view === 'meter' ? 'active' : ''} aria-pressed={view === 'meter'} onClick={() => setView('meter')}><Icon name="activity" size={14} />Meter</button>
        <button className={view === 'capture' ? 'active' : ''} aria-pressed={view === 'capture'} onClick={() => setView('capture')}><Icon name="database" size={14} />Capture</button>
        <button className={view === 'game' ? 'active' : ''} aria-pressed={view === 'game'} onClick={() => setView('game')}><Icon name="sword" size={14} />Game data</button>
      </nav>
      <span className={`capture-pill ${connection !== 'connected' ? 'warn' : status === 'running' ? 'live' : status === 'paused' ? 'warn' : 'off'}`} role="status"><span className="status-dot" />
        {connection !== 'connected' ? 'Reconnecting' : status === 'running' ? 'Recording' : status === 'paused' ? 'Paused' : status === 'completed' ? 'Stopped' : 'Ready'}</span>
      <WindowControls />
    </header>
    <UpdateToast />
    {!data && connection !== 'connected' ? <AppLoading /> : <main className="mini-main">
      {error && <div className="error-banner" role="alert">{error}<button onClick={dismissError}>Dismiss</button></div>}
      {view !== 'capture' && <CaptureNotice openCapture={() => setView('capture')} />}
      {view === 'meter' && <>
        <section className="panel mini-controls">
          <div className="mini-time"><span className="muted">Encounter</span><strong>{duration(live ? data.elapsedSeconds : 0)}</strong></div>
          <div className="mini-buttons">
            {status === 'running' && <button className="button secondary" disabled={!canControl} onClick={() => void command('pause-record')}><Icon name="pause" size={13} />Pause</button>}
            {status === 'paused' && <button className="button primary" disabled={!canControl} onClick={() => void command('resume-record')}><Icon name="play" size={13} />Resume</button>}
            {(status === 'idle' || status === 'completed') && <button className="button primary" disabled={!canControl} onClick={() => void command('start-record')}><Icon name="play" size={13} />Play</button>}
            {(status === 'running' || status === 'paused') && <button className="button secondary" disabled={!canControl} onClick={() => void command('stop-record')}><Icon name="stop" size={13} />Stop</button>}
            {(status === 'running' || status === 'paused') && <button className="button secondary" disabled={!canControl} title="Discard this record and start again from zero" onClick={() => void command('new-record')}><Icon name="history" size={13} />Reset</button>}
          </div>
        </section>
        <AlbionLabCard recording={status === 'running' || status === 'paused'} />
        <section className="panel mini-bars">
          <div className="panel-heading">
            <div className="heading-inline"><h2>{metric === 'damage' ? 'Damage' : 'Healing'}</h2><span className="heading-note">{compact(total)} total · {precise(live && data.elapsedSeconds > 0 ? total / data.elapsedSeconds : 0)} party {unit}</span></div>
            <div className="heading-actions">
              <div className="segmented" aria-label="Metric"><button className={metric === 'damage' ? 'active' : ''} aria-pressed={metric === 'damage'} onClick={() => setMetric('damage')}>Damage</button><button className={metric === 'healing' ? 'active' : ''} aria-pressed={metric === 'healing'} onClick={() => setMetric('healing')}>Healing</button></div>
              {data && <CopyDamage key={`${data.id}-${metric}`} snapshot={data} metric={metric} />}
            </div>
          </div>
          {players.length === 0 ? <p className="mini-empty">{status === 'idle' ? 'Press Play, then fight: party members appear here.' : 'Waiting for combat…'}</p> : <ol className="bars">
            {players.map((player, index) => {
              const value = player[metric];
              const share = metric === 'damage' ? player.damageShare : player.healingShare;
              return <li key={player.id} style={{ '--player-color': player.color, '--bar': `${top > 0 ? value / top * 100 : 0}%` } as CSSProperties}>
                <span className="bar-rank">{index + 1}</span>
                <WeaponIcon player={player} />
                <span className="bar-name">{player.name}</span>
                <span className="bar-value">{compact(value)} <small>({share.toFixed(1)}%)</small></span>
                <span className="bar-rate">{precise(metric === 'damage' ? player.encounterDps : player.encounterHps)} {unit}</span>
              </li>;
            })}
          </ol>}
        </section>
      </>}
      {view === 'capture' && <CapturePanel />}
      {view === 'game' && <GameLibrary tab={catalogTab} onTab={setCatalogTab} />}
    </main>}
  </div>;
}

createRoot(document.getElementById('root')!).render(<Meter />);
