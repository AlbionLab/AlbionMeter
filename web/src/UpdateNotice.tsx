import { useEffect, useState } from 'react';

type Update = { current: string; installed: boolean; state: string; available: string | null; error: string | null };

/** A newer Albion Meter: installed copies restart into the downloaded version, ZIP copies link to the download page. */
export function UpdateNotice() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [restarting, setRestarting] = useState(false);
  useEffect(() => {
    let disposed = false;
    const load = () => fetch('/api/update').then(response => response.ok ? response.json() : null).then(value => { if (!disposed) setUpdate(value); }).catch(() => {});
    void load(); const timer = setInterval(load, 60_000);
    return () => { disposed = true; clearInterval(timer); };
  }, []);
  if (!update?.available) return null;
  if (update.state === 'ready') return <div className="connection-banner update-banner" role="status">
    <span>Albion Meter {update.available} is downloaded. It installs the next time the meter opens, or now: the meter restarts and the current record stops.</span>
    <button className="button primary" disabled={restarting} onClick={() => { setRestarting(true); void fetch('/api/update/apply', { method: 'POST', headers: { 'X-Albion-Meter': 'local' } }); }}>{restarting ? 'Restarting…' : 'Restart and update'}</button>
  </div>;
  if (update.state === 'available') return <div className="connection-banner update-banner" role="status">
    <span>Albion Meter {update.available} is available (you have {update.current}). <a href="https://albionlab.app/#download" target="_blank" rel="noreferrer">Download it at albionlab.app</a>; the Windows installer updates itself from then on.</span>
  </div>;
  return null;
}
