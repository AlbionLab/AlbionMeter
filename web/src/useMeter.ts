import { useEffect, useRef, useState } from 'react';
import { HubConnectionBuilder, LogLevel } from '@microsoft/signalr';
import type { Snapshot } from './types';
import { mergeSnapshot } from './state';

export function useMeter() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [connection, setConnection] = useState('connecting');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const commandPending = useRef(false);
  useEffect(() => {
    let disposed = false;
    let retry: ReturnType<typeof setTimeout>;
    const hub = new HubConnectionBuilder().withUrl('/stream').withAutomaticReconnect([0, 1000, 3000, 5000])
      .configureLogging(LogLevel.Error).build();
    hub.on('Snapshot', (next: Snapshot) => { if (!disposed) setSnapshot(previous => mergeSnapshot(previous, next)); });
    hub.onreconnecting(() => { if (!disposed) setConnection('reconnecting'); });
    hub.onreconnected(() => { if (!disposed) { setConnection('connected'); setError(null); } });
    async function connect() {
      try {
        await hub.start();
        if (disposed) { await hub.stop(); return; }
        setConnection('connected');
        setError(null);
      } catch {
        if (!disposed) { setConnection('disconnected'); retry = setTimeout(connect, 3000); }
      }
    }
    hub.onclose(() => { if (!disposed) { setConnection('disconnected'); retry = setTimeout(connect, 3000); } });
    void connect();
    return () => { disposed = true; clearTimeout(retry); void hub.stop(); };
  }, []);
  async function command(action: string) {
    if (commandPending.current) return;
    commandPending.current = true;
    setBusy(true);
    setError(null);
    try {
      const response = await fetch(`/api/session/${action}`, { method: 'POST', headers: { 'X-Albion-Meter': 'local' } });
      if (!response.ok) throw new Error((await response.json()).error ?? 'The session could not be updated.');
      const next = await response.json() as Snapshot;
      setSnapshot(previous => mergeSnapshot(previous, next));
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'The server is unavailable.'); }
    finally { commandPending.current = false; setBusy(false); }
  }
  return { snapshot, connection, error, busy, command, dismissError: () => setError(null) };
}
