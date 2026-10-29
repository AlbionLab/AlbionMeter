import { useEffect, useState } from 'react';
import { desktop, type UpdateState } from './desktop';

export function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!desktop) return;
    void desktop.info().then(info => setMaximized(info.maximized));
    return desktop.onWindowState(state => setMaximized(state.maximized));
  }, []);
  if (!desktop) return null;
  return <div className="window-controls">
    <button aria-label="Minimize" onClick={() => desktop?.window('minimize')}><svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 5h10" stroke="currentColor" /></svg></button>
    <button aria-label={maximized ? 'Restore' : 'Maximize'} onClick={() => desktop?.window('maximize')}>
      {maximized
        ? <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2.5 2.5V.5h7v7h-2" fill="none" stroke="currentColor" /><rect x=".5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" /></svg>
        : <svg width="10" height="10" viewBox="0 0 10 10"><rect x=".5" y=".5" width="9" height="9" fill="none" stroke="currentColor" /></svg>}
    </button>
    <button className="close" aria-label="Close" onClick={() => desktop?.window('close')}><svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" strokeWidth="1.1" /></svg></button>
  </div>;
}

const megabytes = (value?: number) => value === undefined ? '' : `${(value / 1_048_576).toFixed(1)} MB`;
const releaseNotes = (version: string) => `https://github.com/AlbionLab/AlbionMeter/releases/tag/v${version}`;

export function UpdateToast() {
  const [update, setUpdate] = useState<UpdateState>({ state: 'idle' });
  const [later, setLater] = useState<string | null>(null);
  useEffect(() => {
    if (!desktop) return;
    void desktop.info().then(info => setUpdate(info.update));
    return desktop.onUpdate(setUpdate);
  }, []);
  if (!desktop) return null;
  if (update.state === 'downloading') return <aside className="update-toast" role="status">
    <div className="update-head"><span className="update-dot" /><strong>Downloading Albion Meter {update.version}</strong><span className="update-percent">{update.percent}%</span></div>
    <div className="update-progress"><i style={{ width: `${update.percent}%` }} /></div>
    <span className="update-note">{update.total ? `${megabytes(update.transferred)} of ${megabytes(update.total)} · ` : ''}Keeps running while you play. <a href={releaseNotes(update.version)} target="_blank" rel="noreferrer">What's new</a></span>
  </aside>;
  if ((update.state === 'ready' || update.state === 'installing') && later !== update.version) return <aside className="update-toast ready" role="status">
    <div className="update-head"><span className="update-dot" /><strong>Albion Meter {update.version} is ready</strong></div>
    <span className="update-note">Restarting stops the current record. If you wait, it installs when you close the meter. <a href={releaseNotes(update.version)} target="_blank" rel="noreferrer">What's new</a></span>
    <div className="update-actions">
      <button className="button secondary" disabled={update.state === 'installing'} onClick={() => setLater(update.version)}>Later</button>
      <button className="button primary" disabled={update.state === 'installing'} onClick={() => desktop?.update('install')}>{update.state === 'installing' ? 'Restarting…' : 'Restart and update'}</button>
    </div>
  </aside>;
  return null;
}

export function AppLoading() {
  return <div className="app-loading" role="status">
    <img src="/mark.svg" alt="" />
    <span className="app-loading-spinner" />
    <span>Loading the meter…</span>
  </div>;
}
