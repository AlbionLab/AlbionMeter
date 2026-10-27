import { useEffect, useRef, useState } from 'react';
import type { Metric, Snapshot } from './types';
import { weaponCatalog } from './weapons';

const number = new Intl.NumberFormat('en', { maximumFractionDigits: 0 });

function contributions(snapshot: Snapshot, metric: Metric) {
  return [...new Map([...(snapshot.formerMembers ?? []), ...snapshot.members].map(player => [player.id, player])).values()]
    .sort((a, b) => b[metric] - a[metric] || a.name.localeCompare(b.name));
}

const title = (metric: Metric) => metric === 'damage' ? 'Total damage' : 'Total healing';

function loadIcon(id: string): Promise<HTMLImageElement | null> {
  return new Promise(resolve => {
    const image = new Image();
    const timeout = setTimeout(() => { image.src = ''; resolve(null); }, 10000);
    image.onload = () => { clearTimeout(timeout); resolve(image); };
    image.onerror = () => { clearTimeout(timeout); resolve(null); };
    image.src = `/api/weapon-icon/${encodeURIComponent(id)}`;
  });
}

const decimal = (value: number, digits: number) => value.toLocaleString('pt-BR', { minimumFractionDigits: digits, maximumFractionDigits: digits, useGrouping: false });

export function damageText(snapshot: Snapshot, metric: Metric = 'damage') {
  const players = contributions(snapshot, metric);
  const total = players.reduce((sum, player) => sum + player[metric], 0);
  const unit = metric === 'damage' ? 'DPS' : 'HPS';
  return players.map((player, index) => `${index + 1}. ${player.name}: ${Math.round(player[metric])}(${decimal(total > 0 ? player[metric] / total * 100 : 0, 1)}%)|${decimal(metric === 'damage' ? player.encounterDps : player.encounterHps, 2)} ${unit}`).join('\n');
}

async function damageImage(snapshot: Snapshot, metric: Metric): Promise<Blob> {
  const players = contributions(snapshot, metric);
  const weapons = await weaponCatalog();
  const equipment = await Promise.all(players.map(async player => {
    const weapon = weapons.find(item => `${item.name} · T${item.tier}.${item.enchantment}` === player.weapon);
    return { weapon, image: weapon ? await loadIcon(weapon.id) : null };
  }));
  const canvas = document.createElement('canvas');
  const width = 640;
  const height = 100 + players.length * 76;
  canvas.width = width * 2;
  canvas.height = height * 2;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Image rendering is unavailable.');
  context.scale(2, 2);
  context.fillStyle = '#0a0a0a';
  context.fillRect(0, 0, width, height);
  context.fillStyle = '#e2e5eb';
  context.font = '600 19px "Segoe UI", sans-serif';
  context.fillText(title(metric), 24, 34);
  const maximum = Math.max(1, players[0]?.[metric] ?? 0);
  const total = players.reduce((sum, player) => sum + player[metric], 0);
  players.forEach((player, index) => {
    const y = 65 + index * 76;
    const { weapon, image } = equipment[index];
    context.fillStyle = '#161616';
    context.fillRect(24, y - 15, 48, 48);
    if (image) context.drawImage(image, 24, y - 15, 48, 48);
    else {
      context.fillStyle = '#8e9aa8';
      context.font = '20px "Segoe UI", sans-serif';
      context.fillText('?', 42, y + 17);
    }
    context.font = '10px "Segoe UI", sans-serif';
    context.fillStyle = '#b8c2d2';
    context.textAlign = 'center';
    context.fillText(weapon?.tier ? `T${weapon.tier}.${weapon.enchantment}` : player.weapon === 'Unarmed' ? 'Unarmed' : 'Unknown', 48, y + 47);
    context.textAlign = 'left';
    context.font = '14px "Segoe UI", sans-serif';
    context.fillStyle = '#e2e5eb';
    let name = player.name;
    while (context.measureText(name).width > 310 && name.length > 1) name = name.slice(0, -1);
    context.fillText(name === player.name ? name : `${name}…`, 86, y);
    context.textAlign = 'right';
    context.fillText(number.format(player[metric]), width - 24, y);
    context.font = '11px "Segoe UI", sans-serif';
    context.fillStyle = '#aeb6c2';
    context.fillText(`${(total > 0 ? player[metric] / total * 100 : 0).toFixed(1)}%`, width - 24, y + 18);
    context.textAlign = 'left';
    context.fillStyle = '#1e1e1e';
    context.fillRect(86, y + 29, width - 110, 8);
    context.fillStyle = /^#[0-9a-f]{6}$/i.test(player.color) ? player.color : '#9ebbf3';
    context.fillRect(86, y + 29, (width - 110) * player[metric] / maximum, 8);
  });
  context.font = '13px "Segoe UI", sans-serif';
  context.fillStyle = '#aeb6c2';
  context.fillText('Total', 24, height - 20);
  context.textAlign = 'right';
  context.fillText(number.format(total), width - 24, height - 20);
  return new Promise((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error('Image rendering failed.')), 'image/png'));
}

export function CopyDamage({ snapshot, metric }: { snapshot: Snapshot; metric: Metric }) {
  const [copied, setCopied] = useState<'text' | 'image' | null>(null);
  const [error, setError] = useState('');
  const reset = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (reset.current) clearTimeout(reset.current); }, []);
  const [busy, setBusy] = useState(false);
  const available = contributions(snapshot, metric).length > 0;
  async function copy(format: 'text' | 'image') {
    setBusy(true);
    if (reset.current) clearTimeout(reset.current);
    setCopied(null);
    setError('');
    try {
      if (format === 'text') {
        if (!navigator.clipboard?.writeText) throw new Error('Text clipboard is unavailable in this browser.');
        await navigator.clipboard.writeText(damageText(snapshot, metric));
      } else {
        if (!navigator.clipboard?.write || typeof ClipboardItem === 'undefined') throw new Error('Image clipboard is unavailable in this browser. Use Copy text.');
        await navigator.clipboard.write([new ClipboardItem({ 'image/png': damageImage(snapshot, metric) })]);
      }
      setCopied(format);
      reset.current = setTimeout(() => setCopied(null), 1600);
    } catch (error) {
      setError(error instanceof DOMException && error.name === 'NotAllowedError'
        ? 'Clipboard access was denied. Allow clipboard access and try again.'
        : error instanceof Error ? error.message : 'Could not copy. Try again.');
    } finally { setBusy(false); }
  }
  return <div className="copy-damage"><div className="copy-damage-actions" role="group" aria-label={`Copy total ${metric}`}><span>Copy total {metric}</span><button className="button secondary copy-action" aria-label="Copy text" title={copied === 'text' ? 'Copied' : 'Copy text'} disabled={!available || busy} onClick={() => void copy('text')}>{copied === 'text' ? '✓' : 'Text'}</button><button className="button secondary copy-action" aria-label="Copy image" title={copied === 'image' ? 'Copied' : 'Copy image'} disabled={!available || busy} onClick={() => void copy('image')}>{copied === 'image' ? '✓' : 'Image'}</button></div><span className="sr-only" role="status">{copied ? 'Copied' : ''}</span>{error && <span className="copy-damage-error" role="alert">{error}</span>}</div>;
}
