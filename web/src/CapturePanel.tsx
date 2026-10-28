import { useEffect, useState } from 'react';
import { precise } from './types';
import { Section, StatCard, StatusPill } from './ui';
import { Select } from './Select';
import { desktop } from './desktop';

type CaptureStatus = {
  state: string; source: string; file: string | null; error: string | null; stopReason: string | null;
  library: string | null; adapter: { received: number; dropped: number; interfaceDropped: number } | null;
  report: { frames: number; bytes: number; counters: Record<string, number>; flows: Record<string, number>; combatDecoded: boolean; combatDiagnostics?: Record<string, number> } | null;
};
type DeviceList = { available: boolean; library: string | null; debugEnabled: boolean; selectedDevice?: string; methods?: { id: string; name: string; available: boolean }[]; devices: { name: string; description: string; method: string; flags: number }[]; error: string | null };

export function CapturePanel() {
  const [devices, setDevices] = useState<DeviceList | null>(null);
  const [device, setDevice] = useState('');
  const [method, setMethod] = useState('socket');
  const [status, setStatus] = useState<CaptureStatus | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [granting, setGranting] = useState(false);
  const [host, setHost] = useState<{ executable: string | null; platform: string } | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => { fetch('/api/health').then(response => response.json()).then(setHost).catch(() => setHost(null)); }, []);
  useEffect(() => {
    let disposed = false;
    fetch('/api/capture/devices').then(async response => { if (!response.ok) throw new Error('Could not list capture interfaces.'); return response.json(); })
      .then((value: DeviceList) => { if (!disposed) { setDevices(value); setDevice(current => {
        const selected = value.devices.find(item => item.name === current) ?? value.devices.find(item => item.name === value.selectedDevice) ?? value.devices[0];
        if (selected) setMethod(selected.method);
        return selected?.name ?? '';
      }); } })
      .catch(reason => { if (!disposed) setError(String(reason)); });
    return () => { disposed = true; };
  }, [refresh]);
  useEffect(() => {
    let disposed = false;
    let pending = false;
    async function update() {
      if (pending) return;
      pending = true;
      try {
        const response = await fetch('/api/capture/status');
        if (!response.ok) throw new Error('Capture status is unavailable.');
        const value = await response.json();
        if (!disposed) setStatus(value);
      } catch (reason) { if (!disposed) setError(String(reason)); }
      finally { pending = false; }
    }
    void update(); const timer = setInterval(() => void update(), 1000);
    return () => { disposed = true; clearInterval(timer); };
  }, []);
  async function command(action: string, file?: File) {
    setBusy(true); setError('');
    try {
      let body: BodyInit | undefined;
      const headers: Record<string, string> = { 'X-Albion-Meter': 'local' };
      if (file) {
        if (file.size > 40_000_000) throw new Error('Replay files are limited to 40 MB.');
        const form = new FormData(); form.append('capture', file); body = form;
      } else if (action === 'start') { headers['Content-Type'] = 'application/json'; body = JSON.stringify({ device, method }); }
      const response = await fetch(`/api/capture/${action}`, { method: 'POST', headers, body });
      const value = await response.json();
      if (!response.ok) throw new Error(value.error ?? 'Capture operation failed.');
      setStatus(value);
    } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setBusy(false); }
  }
  async function grant() {
    setGranting(true); setError('');
    try {
      if (!await desktop!.permission()) { setError('Packet capture permission was not granted.'); return; }
      setRefresh(value => value + 1);
      if (device) await command('start');
    } finally { setGranting(false); }
  }
  const active = status && ['starting', 'running', 'stopping'].includes(status.state);
  const failure = `${error} ${status?.error ?? ''} ${devices?.error ?? ''}`;
  const permissionHint = /CAP_NET_RAW|permission|npcap|wpcap/i.test(failure);
  const setcap = `sudo setcap cap_net_raw=ep ${host?.executable ?? './AlbionMeter.Server'}`;
  const report = status?.report;
  // The card with the default route carries the game traffic; every-interface capture also follows ExitLag, NoPing and VPN adapters.
  const everything = (name: string) => name === 'raw:all' || name === 'raw:any';
  const deviceOptions = (devices?.devices ?? []).filter(item => item.method === method)
    .map(item => ({ value: item.name, label: item.description || item.name, badge: everything(item.name) ? 'ExitLag / VPN' : (item.flags & 1) !== 0 ? 'Main' : undefined }))
    .sort((a, b) => Number(b.badge === 'Main') - Number(a.badge === 'Main'));
  const diagnostics = report ? Object.entries({ ...report.counters, ...report.combatDiagnostics }) : [];
  return <div className="capture-layout">
    <section className="panel capture-panel">
      <div className="capture-head">
        <div><span className="eyebrow">PACKET CAPTURE & REPLAY</span><h2>Observe the connection.</h2><p>Observe the network or analyze a saved PCAP / PCAPNG. Starting an interface does not start an encounter; press Play in Overview to record combat.</p></div>
        <div className="capture-state"><StatusPill status={status?.state ?? 'connecting'} /><span>{status?.source ?? 'none'}</span></div>
      </div>
      <div className="capture-controls">
        <div className="capture-source">
        <Select className="capture-select" aria-label="Capture method" disabled={busy || !!active} value={method} onChange={next => { setMethod(next); setDevice(devices?.devices.find(item => item.method === next)?.name ?? ''); }} options={devices?.methods?.map(item => ({ value: item.id, label: `${item.name}${item.available ? '' : ' (unavailable)'}`, disabled: !item.available })) ?? []} />
        <Select className="capture-select" aria-label="Capture interface" disabled={busy || !!active} value={device} onChange={setDevice} placeholder="Select network interface" options={deviceOptions} />
        <button className="button secondary" disabled={busy || !!active} onClick={() => setRefresh(value => value + 1)}>Refresh</button>
        </div>
        <div className="capture-actions">
        <button className="button primary" disabled={busy || !!active || !device} onClick={() => void command('start')}>Start capture</button>
        <button className="button secondary" disabled={!active || status?.state === 'stopping'} onClick={() => void command('stop')}>Stop</button>
        <label className={`button secondary ${busy || active ? 'disabled' : ''}`}>Replay capture<input className="folder-input" type="file" accept=".pcap,.pcapng,.cap" disabled={busy || !!active} onChange={event => { const file = event.target.files?.[0]; if (file) void command('replay', file); event.target.value = ''; }} /></label>
        </div>
      </div>
      {devices?.error && <p className="capture-note" role="status">Capture interface access: {devices.error}</p>}
      {(error || status?.error) && <p className="error-banner" role="alert">{error || status?.error}</p>}
      {permissionHint && <div className="capture-hint">{host?.platform === 'windows'
        ? <p>Close Albion Meter and open it again as administrator (right-click AlbionMeter.Server.exe, Run as administrator). Packet capture uses a Windows raw socket and needs no driver.</p>
        : desktop ? <div className="capture-hint-grant"><p>Albion Meter needs permission to read the game traffic. Allow it once with your system password and capture starts right away.</p><button className="button primary" disabled={granting} onClick={() => void grant()}>{granting ? 'Waiting for permission…' : 'Allow packet capture'}</button></div>
        : <><p>Grant packet capture permission once, then restart Albion Meter:</p><div className="capture-hint-command"><code>{setcap}</code><button className="button secondary" onClick={() => void navigator.clipboard?.writeText(setcap).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); })}>{copied ? 'Copied' : 'Copy'}</button></div><p className="capture-hint-note">Rebuilding or replacing the executable removes this permission.</p></>}</div>}
    </section>
    <div className="metric-grid four">
      <StatCard icon="activity" tone="orange" label="Captured frames" value={precise(report?.frames ?? 0)} />
      <StatCard icon="layers" tone="blue" label="Transport messages" value={precise(report?.counters.transportMessages ?? 0)} />
      <StatCard icon="database" tone="violet" label="Processed traffic" value={report ? `${(report.bytes / 1_048_576).toFixed(1)} MB` : '—'} />
      <StatCard icon="info" tone="red" label="Adapter drops" value={status?.adapter ? precise(status.adapter.dropped) : '—'} />
    </div>
    <div className="capture-columns">
      <Section title="Diagnostics" count={diagnostics.length} note="Decoder counters for the current capture">
        {diagnostics.length ? <dl className="diag-grid">{diagnostics.map(([name, value]) => <div key={name}><dt>{name}</dt><dd>{precise(value)}</dd></div>)}</dl> : <div className="empty-list">No capture report yet.</div>}
      </Section>
      <Section title="Session">
        <dl className="side-list"><div><dt>Library</dt><dd>{status?.library ?? devices?.library ?? 'Unavailable'}</dd></div><div><dt>Combat decoding</dt><dd>Live combat</dd></div>{status?.stopReason && <div><dt>Stopped</dt><dd>{status.stopReason}</dd></div>}</dl>
        {status?.file && <p className="side-note storage-path">{status.file}</p>}
        <dl className="side-list"><div><dt>Packet persistence</dt><dd>{devices?.debugEnabled ? 'Debug · packets table (bounded)' : 'Off · combat data only'}</dd></div></dl>
        <p className="side-note">Live capture does not create PCAP files. The database stores combat timelines, contributions, players, party membership and equipment used by the meter. Replay uses a temporary file which is deleted after analysis.</p>
        <p className="side-note">Choose a system socket or {host?.platform === 'windows' ? 'Npcap' : 'libpcap'}. Stop capture before changing the method. Socket capture needs administrator rights on Windows or CAP_NET_RAW on Linux.</p>
        {devices?.debugEnabled && <p className="side-note">Debug mode stores at most 10,000 packets or 32 MB of packet payloads in the packets table. Set Capture__Debug=false and restart to disable it.</p>}
      </Section>
    </div>
  </div>;
}
