import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import dgram from 'node:dgram';
import { chromium } from 'playwright';

const root = fileURLToPath(new URL('../../', import.meta.url));
const base = 'http://127.0.0.1:5187';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const post = async (path, body) => {
  const response = await fetch(`${base}${path}`, { method: 'POST', headers: { 'X-Albion-Meter': 'local', 'Content-Type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
  return { status: response.status, body: await response.json() };
};
async function send() {
  const sender = dgram.createSocket('udp4');
  try { await new Promise((resolve, reject) => sender.send(Buffer.from([1, 2, 3, 4]), 5056, '127.0.0.1', error => error ? reject(error) : resolve())); }
  finally { sender.close(); }
}
for (const debug of [false, true]) {
  const directory = await mkdtemp(join(tmpdir(), 'albion-capture-storage-'));
  const executable = process.env.ALBION_SERVER ?? join(root, 'src/AlbionMeter.Server/bin/Debug/net10.0/AlbionMeter.Server');
  const server = spawn(executable, [`--Storage:Directory=${directory}`, '--Capture:AutoStart=false'], { cwd: root, env: { ...process.env, Capture__Debug: String(debug) }, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = ''; server.stdout.on('data', chunk => { output += chunk; }); server.stderr.on('data', chunk => { output += chunk; });
  let browser;
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (server.exitCode !== null) throw new Error(output);
      try { if ((await fetch(`${base}/api/health`)).ok) break; } catch { }
      await sleep(100);
    }
    const devices = await (await fetch(`${base}/api/capture/devices`)).json();
    assert.equal(devices.debugEnabled, debug);
    assert.ok(devices.methods.some(method => method.id === 'socket' && method.available));
    assert.ok(devices.methods.some(method => method.id === 'pcap'));
    assert.equal((await post('/api/capture/start', { method: 'pcap', device: 'raw:any' })).status, 400);
    const methods = ['socket', ...(debug ? [] : ['pcap'])];
    for (const method of methods) {
      const device = devices.devices.find(device => device.method === method && (method === 'socket' ? device.name === 'raw:any' : device.name === 'lo'));
      if (!device) continue;
      assert.equal((await post('/api/capture/start', { method, device: device.name })).status, 200);
      let status;
      for (let attempt = 0; attempt < 60; attempt++) {
        await send(); await sleep(100);
        status = await (await fetch(`${base}/api/capture/status`)).json();
        if (status.state === 'failed') throw new Error(status.error);
        if (status.report?.frames > 0) break;
      }
      assert.equal(status.state, 'running'); assert.ok(status.report.frames > 0); assert.equal(status.file, null);
      await post('/api/capture/stop');
      console.log(`PASS ${method} capture with debug=${debug}: receives Albion-port traffic without PCAP files`);
    }
    browser = await chromium.launch();
    const page = await browser.newPage();
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto(base);
    await page.getByRole('button', { name: 'Capture', exact: true }).click();
    const method = page.getByRole('button', { name: 'Capture method', exact: true }); await method.waitFor();
    await method.click(); await page.getByRole('option', { name: /System socket/ }).click();
    await page.getByRole('button', { name: 'Capture interface', exact: true }).click();
    assert.ok((await page.getByRole('option').allTextContents()).some(text => text.includes('socket')));
    await page.keyboard.press('Escape');
    await page.getByText(debug ? 'Debug · packets table (bounded)' : 'Off · combat data only', { exact: true }).waitFor();
    assert.deepEqual(errors, []);
    assert.ok(!(await readdir(join(directory, 'captures'))).some(file => /\.pcap|\.jsonl/.test(file)));
    await browser.close(); browser = undefined;
    server.kill('SIGTERM'); await new Promise(resolve => server.once('exit', resolve));
    const result = JSON.parse(execFileSync('python3', ['-c', `import sqlite3,json,sys
c=sqlite3.connect(sys.argv[1]); exists=c.execute("select count(*) from sqlite_master where type='table' and name='packets'").fetchone()[0]
print(json.dumps({'exists':exists,'rows':c.execute('select count(*) from packets').fetchone()[0] if exists else 0}))`, join(directory, 'encounters.db')], { encoding: 'utf8' }));
    assert.equal(result.exists, debug ? 1 : 0);
    if (debug) assert.ok(result.rows > 0);
    console.log(`PASS debug=${debug}: packet table ${debug ? 'stores packets' : 'does not exist'}, UI selector and persistence status`);
  } finally {
    if (browser) await browser.close();
    if (server.exitCode === null) { server.kill('SIGTERM'); await new Promise(resolve => server.once('exit', resolve)); }
    await rm(directory, { recursive: true, force: true });
  }
}
