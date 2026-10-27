import { useRef, useState } from 'react';

async function responseError(response: Response) {
  const body = await response.json().catch(() => null);
  return body?.error ?? `Request failed (${response.status}).`;
}

export function HistoryExport({ encounterId, dungeonId, label = 'Export' }: { encounterId?: string; dungeonId?: string; label?: string }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function download() {
    setBusy(true); setError('');
    try {
      const query = new URLSearchParams();
      if (encounterId) query.set('encounterId', encounterId);
      if (dungeonId) query.set('dungeonId', dungeonId);
      const response = await fetch(`/api/history/export?${query}`);
      if (!response.ok) throw new Error(await responseError(response));
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement('a'); link.href = url;
      link.download = response.headers.get('content-disposition')?.match(/filename="?([^";]+)/)?.[1] ?? 'albion-history.zip';
      document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (failure) { setError(failure instanceof Error ? failure.message : 'Could not export history.'); }
    finally { setBusy(false); }
  }
  return <span className="history-export"><button className="button secondary" disabled={busy} onClick={() => void download()}>{busy ? 'Exporting…' : label}</button>{error && <span className="history-transfer-error" role="alert">{error}<button onClick={() => setError('')} aria-label="Dismiss export error">×</button></span>}</span>;
}

export function HistoryTransfer({ onImported }: { onImported: () => void }) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState(false);
  async function upload(file: File) {
    setBusy(true); setMessage(''); setError(false);
    try {
      if (file.size > 40_000_000) throw new Error('Choose a history file smaller than 40 MB.');
      const form = new FormData(); form.append('history', file);
      const response = await fetch('/api/history/import', { method: 'POST', headers: { 'X-Albion-Meter': 'local' }, body: form });
      if (!response.ok) throw new Error(await responseError(response));
      const result = await response.json();
      const skipped = result.skippedEncounters + result.skippedDungeons;
      setMessage(`Imported ${result.importedEncounters} encounters and ${result.importedDungeons} dungeons.${skipped ? ` ${skipped} existing records or records over the storage limit were skipped.` : ''}`);
      onImported();
    } catch (failure) { setError(true); setMessage(failure instanceof Error ? failure.message : 'Could not import history.'); }
    finally { setBusy(false); if (input.current) input.current.value = ''; }
  }
  return <div className="history-transfer"><div className="heading-actions"><HistoryExport label="Export history" /><button className="button secondary" disabled={busy} onClick={() => input.current?.click()}>{busy ? 'Importing…' : 'Import history'}</button><input ref={input} type="file" accept=".zip,.json" hidden aria-label="Import history file" onChange={event => { const file = event.target.files?.[0]; if (file) void upload(file); }} /></div>{message && <div className={error ? 'error-banner' : 'history-transfer-result'} role={error ? 'alert' : 'status'}>{message}<button onClick={() => setMessage('')} aria-label="Dismiss import result">×</button></div>}</div>;
}
