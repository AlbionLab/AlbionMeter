import type { ReactNode } from 'react';
import { Icon, type IconName } from './Icon';

export function StatCard({ icon, tone, label, value, note }: { icon: IconName; tone: string; label: string; value: string; note?: string }) {
  return <div className="metric-card"><span className={`metric-icon ${tone}`}><Icon name={icon} size={18} /></span><div><div className="metric-caption">{label}</div><div className="metric-value">{value}</div>{note && <div className="metric-note">{note}</div>}</div></div>;
}

export function Section({ title, note, count, actions, className = '', children }: { title: ReactNode; note?: ReactNode; count?: number; actions?: ReactNode; className?: string; children?: ReactNode }) {
  return <section className={`panel ${className}`}>
    <div className="panel-heading"><div className="heading-inline"><h2>{title}</h2>{count !== undefined && <span className="count-badge">{count}</span>}{note && <span className="heading-note">{note}</span>}</div>{actions && <div className="heading-actions">{actions}</div>}</div>
    {children}
  </section>;
}

export function ShareBar({ value, color = 'var(--accent)' }: { value: number; color?: string }) {
  return <div className="share-cell"><div><i style={{ width: `${Math.min(100, Math.max(0, value))}%`, background: color }} /></div><span>{value.toFixed(1)}%</span></div>;
}

export function StatusPill({ status }: { status: string }) {
  const tone = ['running', 'live', 'starting'].includes(status) ? 'good' : ['paused', 'stopping'].includes(status) ? 'warn' : ['failed', 'error'].includes(status) ? 'bad' : 'neutral';
  return <span className={`status-pill ${tone}`}>{status}</span>;
}
