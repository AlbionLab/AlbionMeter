import { useEffect, useState } from 'react';
import { precise } from './types';
import { t } from './i18n';

type Status = { state: string; source: string; error: string | null; stopReason: string | null; report: { combatDiagnostics?: Record<string, number> } | null };
const reasons: Record<string, string> = { storageLimit: 'the capture storage limit was reached', sourceEnded: 'the network interface stopped delivering packets', error: 'an error occurred' };

export function CaptureNotice({ openCapture }: { openCapture: () => void }) {
  const [status, setStatus] = useState<Status | null>(null);
  useEffect(() => {
    let disposed = false;
    const update = () => fetch('/api/capture/status').then(response => response.ok ? response.json() : null).then(value => { if (!disposed) setStatus(value); }).catch(() => {});
    void update(); const timer = setInterval(update, 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, []);
  if (!status || status.source === 'replay') return null;
  const pending = status.report?.combatDiagnostics?.pendingUnidentified ?? 0;
  const recording = ['starting', 'running'].includes(status.state);
  const permission = /permission|CAP_NET_RAW|npcap/i.test(status.error ?? '');
  const english = permission ? 'packet capture permission is missing' : status.stopReason ? reasons[status.stopReason] : status.source === 'none' ? 'it was not started' : undefined;
  const reason = english && t(english);
  return <>
    {status.state === 'failed' && <div className="error-banner capture-alert" role="alert"><span><strong>{t('Combat is not being recorded')}</strong>{reason ? t(' because {reason}', { reason }) : ''}{t('. Damage and healing from now on will be lost.')}</span><button className="button primary" onClick={openCapture}>{t('Open Capture')}</button></div>}
    {recording && pending >= 30 && <div className="connection-banner" role="status"><span>{t('{n} combat updates are waiting for their players to be identified. They are credited as soon as each player is named (chat message, gear change or coming into view) and kept as unidentified players at the next zone change.', { n: precise(pending) })}</span></div>}
  </>;
}
