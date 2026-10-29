import { useEffect, useRef, useState } from 'react';
import { Icon, type IconName } from './Icon';
import type { Metric, Snapshot } from './types';
import { weaponCatalog } from './weapons';

const number = () => new Intl.NumberFormat('en', { maximumFractionDigits: 0 });

function contributions(snapshot: Snapshot, metric: Metric) {
  return [...new Map([...(snapshot.formerMembers ?? []), ...snapshot.members].map(player => [player.id, player])).values()]
    .sort((a, b) => b[metric] - a[metric] || a.name.localeCompare(b.name));
}

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

/** One ranked line per player: "1. Name: 1747647(14,4%)|1234,56 DPS", the format other Albion meters share in chat. */
export function damageText(snapshot: Snapshot, metric: Metric = 'damage') {
  const players = contributions(snapshot, metric);
  const total = players.reduce((sum, player) => sum + player[metric], 0);
  const unit = metric === 'damage' ? 'DPS' : 'HPS';
  return players.map((player, index) => `${index + 1}. ${player.name}: ${Math.round(player[metric])}(${decimal(total > 0 ? player[metric] / total * 100 : 0, 1)}%)|${decimal(metric === 'damage' ? player.encounterDps : player.encounterHps, 2)} ${unit}`).join('\n');
}

/** The players' names in ranking order, one per line. */
export function playerNames(snapshot: Snapshot, metric: Metric = 'damage') {
  return contributions(snapshot, metric).map(player => player.name).join('\n');
}

function loadImage(source: string): Promise<HTMLImageElement | null> {
  return new Promise(resolve => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => resolve(null);
    image.src = source;
  });
}

const compactNumber = (value: number) => value >= 1e6 ? `${(value / 1e6).toFixed(2)}M` : value >= 1e3 ? `${(value / 1e3).toFixed(1)}K` : number().format(Math.round(value));
const clock = (seconds: number) => {
  const whole = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(whole / 3600), minutes = Math.floor(whole / 60) % 60, rest = whole % 60;
  const pad = (value: number) => String(value).padStart(2, '0');
  return hours ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${pad(minutes)}:${pad(rest)}`;
};

/**
 * The ranking as a picture, drawn like the session link preview on Discord: the AlbionLab brand, the party totals, then
 * one row per player whose background bar is their share of the leader, with weapon, value, share and DPS.
 */
async function damageImage(snapshot: Snapshot, metric: Metric): Promise<Blob> {
  const players = contributions(snapshot, metric);
  const weapons = await weaponCatalog();
  const [brand, ...equipment] = await Promise.all([loadImage('/brand.svg'), ...players.map(async player => {
    const weapon = weapons.find(item => `${item.name} · T${item.tier}.${item.enchantment}` === player.weapon);
    return { weapon, image: weapon ? await loadIcon(weapon.id) : null };
  })]) as [HTMLImageElement | null, ...{ weapon: Awaited<ReturnType<typeof weaponCatalog>>[number] | undefined; image: HTMLImageElement | null }[]];
  const width = 900, pad = 40, rowHeight = 46, rowGap = 8, top = 286;
  const height = top + players.length * (rowHeight + rowGap) + 56;
  const canvas = document.createElement('canvas');
  canvas.width = width * 2;
  canvas.height = height * 2;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Image rendering is unavailable.');
  context.scale(2, 2);
  const sans = '"Noto Sans", "Segoe UI", system-ui, sans-serif', mono = '"JetBrains Mono", ui-monospace, Consolas, monospace';
  const font = (weight: number, size: number, family = sans) => { context.font = `${weight} ${size}px ${family}`; };
  const rounded = (x: number, y: number, w: number, h: number, r: number) => { context.beginPath(); context.roundRect(x, y, w, h, r); };
  const fit = (text: string, max: number) => {
    if (context.measureText(text).width <= max) return text;
    let cut = text;
    while (cut.length > 1 && context.measureText(`${cut}…`).width > max) cut = cut.slice(0, -1);
    return `${cut.trimEnd()}…`;
  };

  // Background with the brand glow.
  context.fillStyle = '#0a0d13';
  context.fillRect(0, 0, width, height);
  const glow = context.createRadialGradient(110, -40, 0, 110, -40, 620);
  glow.addColorStop(0, '#f987142e');
  glow.addColorStop(1, '#f9871400');
  context.fillStyle = glow;
  context.fillRect(0, 0, width, height);

  // Brand and what the picture is.
  if (brand) context.drawImage(brand, pad, 32, 40 * brand.width / Math.max(1, brand.height), 40);
  font(700, 24);
  context.fillStyle = '#e9ecf2';
  context.fillText('AlbionLab', pad + 56, 61);
  const pill = (metric === 'damage' ? 'Total damage' : 'Total healing').toUpperCase();
  font(700, 12);
  const pillWidth = context.measureText(pill).width + 28;
  context.fillStyle = '#f9871422';
  rounded(width - pad - pillWidth, 36, pillWidth, 30, 15);
  context.fill();
  context.fillStyle = '#f98714';
  context.textAlign = 'center';
  context.fillText(pill, width - pad - pillWidth / 2, 55);
  context.textAlign = 'left';

  // Title line: when, how long, how many.
  font(800, 40);
  context.fillStyle = '#e9ecf2';
  context.fillText(metric === 'damage' ? 'Party damage' : 'Party healing', pad, 132);
  const started = new Date(snapshot.startedAt);
  const when = Number.isNaN(started.getTime()) ? '' : started.toLocaleString('en', { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
  font(400, 17);
  context.fillStyle = '#808a9b';
  context.fillText([when, clock(snapshot.elapsedSeconds), `${players.length} ${(players.length === 1 ? 'player' : 'players')}`].filter(Boolean).join('   ·   '), pad + 2, 164);

  // Party totals.
  const totalDamage = players.reduce((sum, player) => sum + player.damage, 0);
  const totalHealing = players.reduce((sum, player) => sum + player.healing, 0);
  const elapsed = Math.max(1, snapshot.elapsedSeconds);
  const stats: [string, string][] = metric === 'damage'
    ? [['DAMAGE', compactNumber(totalDamage)], ['HEALING', compactNumber(totalHealing)], ['PARTY DPS', number().format(Math.round(totalDamage / elapsed))]]
    : [['HEALING', compactNumber(totalHealing)], ['DAMAGE', compactNumber(totalDamage)], ['PARTY HPS', number().format(Math.round(totalHealing / elapsed))]];
  const cardWidth = (width - pad * 2 - 24) / 3;
  stats.forEach(([label, value], index) => {
    const x = pad + index * (cardWidth + 12);
    context.fillStyle = '#11161f';
    rounded(x, 190, cardWidth, 72, 12);
    context.fill();
    context.strokeStyle = '#1f2734';
    context.lineWidth = 1;
    context.stroke();
    font(700, 11);
    context.fillStyle = '#808a9b';
    context.fillText(label, x + 18, 214);
    font(600, 26, mono);
    context.fillStyle = '#e9ecf2';
    context.fillText(value, x + 18, 248);
  });

  // The ranking.
  const total = players.reduce((sum, player) => sum + player[metric], 0);
  const leader = Math.max(1, players[0]?.[metric] ?? 0);
  const unit = metric === 'damage' ? 'DPS' : 'HPS';
  players.forEach((player, index) => {
    const y = top + index * (rowHeight + rowGap);
    const x = pad, w = width - pad * 2;
    const color = /^#[0-9a-f]{6}$/i.test(player.color) ? player.color : '#9ebbf3';
    context.fillStyle = '#161c27';
    rounded(x, y, w, rowHeight, 9);
    context.fill();
    context.save();
    rounded(x, y, w, rowHeight, 9);
    context.clip();
    context.fillStyle = `${color}4a`;
    context.fillRect(x, y, w * player[metric] / leader, rowHeight);
    context.restore();
    context.strokeStyle = '#1f2734';
    rounded(x, y, w, rowHeight, 9);
    context.stroke();
    const middle = y + rowHeight / 2;
    font(600, 14, mono);
    context.fillStyle = '#808a9b';
    context.textAlign = 'center';
    context.fillText(String(index + 1), x + 22, middle + 5);
    context.textAlign = 'left';
    const { weapon, image } = equipment[index];
    if (image) context.drawImage(image, x + 42, y + 5, 36, 36);
    else {
      context.fillStyle = '#0a0d13';
      rounded(x + 42, y + 5, 36, 36, 6);
      context.fill();
    }
    const right = x + w - 18;
    font(400, 13, mono);
    const rate = `${number().format(Math.round(metric === 'damage' ? player.encounterDps : player.encounterHps))} ${unit}`;
    context.fillStyle = '#b7becb';
    context.textAlign = 'right';
    context.fillText(rate, right, middle + 5);
    font(600, 15, mono);
    context.fillStyle = '#e9ecf2';
    const amount = `${compactNumber(player[metric])}  (${(total > 0 ? player[metric] / total * 100 : 0).toFixed(1)}%)`;
    const amountRight = right - 96;
    context.fillText(amount, amountRight, middle + 5);
    const amountLeft = amountRight - context.measureText(amount).width - 20;
    context.textAlign = 'left';
    font(700, 17);
    const name = fit(player.name, Math.min(280, amountLeft - (x + 92)));
    context.save();
    context.fillStyle = '#e9ecf2';
    context.fillText(name, x + 92, middle + 6);
    context.restore();
    const nameEnd = x + 92 + context.measureText(name).width + 12;
    const tier = weapon?.tier ? `T${weapon.tier}.${weapon.enchantment}` : '';
    const weaponLabel = weapon ? `${weapon.name}${tier ? ` · ${tier}` : ''}` : player.weapon === 'Unarmed' ? 'Unarmed' : '';
    if (weaponLabel && amountLeft - nameEnd > 60) {
      font(400, 13);
      context.fillStyle = '#9aa3b2';
      context.fillText(fit(weaponLabel, amountLeft - nameEnd), nameEnd, middle + 5);
    }
  });

  // Footer.
  const footer = height - 24;
  font(400, 14);
  context.fillStyle = '#808a9b';
  context.fillText(`${'Total'}  ${number().format(Math.round(total))}`, pad + 2, footer);
  context.textAlign = 'right';
  context.fillStyle = '#f98714';
  context.fillText('albionlab.app', width - pad, footer);
  return new Promise((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error('Image rendering failed.')), 'image/png'));
}

type Format = 'text' | 'image' | 'names';

const formats: { format: Format; icon: IconName; label: string; hint: string; done: string }[] = [
  { format: 'image', icon: 'image', label: 'Ranking as image', hint: 'The ranking with weapons and bars, ready to paste', done: 'Ranking copied as image' },
  { format: 'text', icon: 'list', label: 'Ranking as text', hint: 'One line per player, the format other meters share in chat', done: 'Ranking copied as text' },
  { format: 'names', icon: 'users', label: 'Player names', hint: 'Only the names, one per line, in ranking order', done: 'Player names copied' },
];

/**
 * One "Copy" button with a menu of what to copy (the ranking as an image or as text, or just the names). The button keeps
 * its size whatever happens: while the image renders and after copying only its icon changes, and the confirmation
 * floats under it, so nothing around it moves or blinks.
 */
export function CopyDamage({ snapshot, metric }: { snapshot: Snapshot; metric: Metric }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<Format | null>(null);
  const [copied, setCopied] = useState<Format | null>(null);
  const [error, setError] = useState('');
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const reset = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (reset.current) clearTimeout(reset.current); }, []);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { setOpen(false); trigger.current?.focus(); } };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    root.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape); };
  }, [open]);
  const available = contributions(snapshot, metric).length > 0;
  function flash(format: Format | null, message = '') {
    if (reset.current) clearTimeout(reset.current);
    setCopied(format);
    setError(message);
    reset.current = setTimeout(() => { setCopied(null); setError(''); }, message ? 4000 : 1800);
  }
  async function copy(format: Format) {
    setOpen(false);
    trigger.current?.focus();
    if (busy || !available) return;
    setBusy(format);
    try {
      if (format === 'image') {
        if (!navigator.clipboard?.write || typeof ClipboardItem === 'undefined') throw new Error('Image clipboard is unavailable in this browser. Use Copy text.');
        await navigator.clipboard.write([new ClipboardItem({ 'image/png': damageImage(snapshot, metric) })]);
      } else {
        if (!navigator.clipboard?.writeText) throw new Error('Text clipboard is unavailable in this browser.');
        await navigator.clipboard.writeText(format === 'names' ? playerNames(snapshot, metric) : damageText(snapshot, metric));
      }
      flash(format);
    } catch (error) {
      flash(null, error instanceof DOMException && error.name === 'NotAllowedError'
        ? 'Clipboard access was denied. Allow clipboard access and try again.'
        : error instanceof Error ? error.message : 'Could not copy. Try again.');
    } finally { setBusy(null); }
  }
  const done = formats.find(item => item.format === copied);
  return <div ref={root} className="copy-damage">
    <button ref={trigger} className={`button secondary copy-trigger${copied ? ' copied' : ''}`} aria-haspopup="menu" aria-expanded={open} aria-disabled={!available}
      title={`Copy total ${metric}`} onClick={() => { if (available && !busy) setOpen(value => !value); }}>
      <span className={`copy-trigger-icon${busy ? ' busy' : ''}`}><Icon name={copied ? 'check' : 'copy'} size={15} /></span>{'Copy'}<Icon name="chevron" size={13} />
    </button>
    {open && <div className="copy-menu" role="menu" aria-label={`Copy total ${metric}`} onKeyDown={event => {
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      event.preventDefault();
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      items[(index + (event.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
    }}>
      <span className="copy-menu-title">{`Copy total ${metric}`}</span>
      {formats.map(item => <button key={item.format} role="menuitem" onClick={() => void copy(item.format)}>
        <Icon name={item.icon} size={16} /><span><strong>{item.label}</strong><small>{item.hint}</small></span>
      </button>)}
    </div>}
    <span className={`copy-toast${done || error || busy === 'image' ? ' shown' : ''}${error ? ' error' : ''}`} role={error ? 'alert' : 'status'}>
      {error || (done ? done.done : busy === 'image' ? 'Preparing image…' : '')}
    </span>
  </div>;
}
