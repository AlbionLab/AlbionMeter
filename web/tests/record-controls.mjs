import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const root = fileURLToPath(new URL('../../', import.meta.url));
const directory = await mkdtemp(join(tmpdir(), 'albion-record-controls-'));
const base = 'http://127.0.0.1:5187';
const server = spawn(join(root, 'src/AlbionMeter.Server/bin/Debug/net10.0/AlbionMeter.Server'), [`--Storage:Directory=${directory}`, '--Capture:AutoStart=false'], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
let output = ''; server.stdout.on('data', data => { output += data; }); server.stderr.on('data', data => { output += data; });
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const json = async path => (await fetch(`${base}${path}`)).json();
const post = async (path, body) => fetch(`${base}${path}`, { method: 'POST', headers: { 'X-Albion-Meter': 'local', 'Content-Type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
let browser;
try {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server.exitCode !== null) throw new Error(output);
    try { if ((await fetch(`${base}/api/health`)).ok) break; } catch { }
    await sleep(100);
  }
  assert.equal((await json('/api/capture/status')).state, 'idle');
  assert.equal((await json('/api/encounters')).length, 0);
  const devices = await json('/api/capture/devices');
  const device = devices.devices.find(value => value.name === 'raw:any');
  assert.equal((await post('/api/capture/start', { method: 'socket', device: device.name })).status, 200);
  for (let i = 0; i < 50 && (await json('/api/capture/status')).state === 'starting'; i++) await sleep(100);
  const idle = await json('/api/session');
  assert.equal(idle.status, 'idle'); assert.equal(idle.elapsedSeconds, 0); assert.equal(idle.eventCount, 0);
  assert.equal((await post('/api/session/resume-record')).status, 409);
  browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(base);
  await sleep(500);
  const startResponse = page.waitForResponse(response => response.url().endsWith('/api/session/start-record'));
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  const startResult = await startResponse; assert.equal(startResult.status(), 200);
  await page.getByRole('button', { name: 'Pause', exact: true }).waitFor();
  const first = await json('/api/session'); assert.equal(first.status, 'running');
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await page.getByRole('button', { name: 'Resume', exact: true }).waitFor();
  const paused = await json('/api/session'); assert.equal(paused.id, first.id); assert.equal(paused.status, 'paused');
  await sleep(300); assert.equal((await json('/api/session')).elapsedSeconds, paused.elapsedSeconds);
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await page.getByRole('button', { name: 'Pause', exact: true }).waitFor();
  assert.equal((await json('/api/session')).id, first.id);
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.getByRole('button', { name: 'Play', exact: true }).waitFor();
  assert.equal((await json('/api/capture/status')).state, 'completed');
  assert.equal((await json('/api/session')).status, 'completed');
  assert.ok((await json('/api/encounters')).some(record => record.id === first.id));
  assert.equal((await post('/api/session/resume-record')).status, 409);
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await page.getByRole('button', { name: 'Pause', exact: true }).waitFor();
  const next = await json('/api/session'); assert.notEqual(next.id, first.id);
  assert.equal(next.eventCount, 0); assert.equal(next.totalDamage, 0); assert.equal(next.totalHealing, 0);
  assert.equal(next.weaponContributions.length, 0); assert.equal(next.itemContributions.length, 0);
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.getByRole('button', { name: 'Play', exact: true }).waitFor();
  assert.equal((await json('/api/encounters')).length, 2);
  assert.deepEqual(errors, []);
  console.log('PASS capture alone remains idle; Play/Pause/Resume/Stop UI; network shutdown; fresh encounter IDs; saved history; zero totals and contributions');
} finally {
  if (browser) await browser.close();
  if (server.exitCode === null) { server.kill('SIGTERM'); await new Promise(resolve => server.once('exit', resolve)); }
  await rm(directory, { recursive: true, force: true });
}
