import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

// Each check holds a status poll (or the first load) open with page.route until the user's action has been answered,
// so its late answer arrives after the action, the order that used to undo it. No packet capture is needed: the capture, AlbionLab and
// record endpoints are answered by the page routes; the server only serves the page and the SignalR stream.
const root = fileURLToPath(new URL('../../', import.meta.url));
const directory = await mkdtemp(join(tmpdir(), 'albion-race-guards-'));
const base = 'http://127.0.0.1:5187';
const server = spawn(process.env.ALBION_SERVER ?? join(root, 'src/AlbionMeter.Server/bin/Debug/net10.0/AlbionMeter.Server'), [`--Storage:Directory=${directory}`, '--Capture:AutoStart=false'], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
let output = ''; server.stdout.on('data', data => { output += data; }); server.stderr.on('data', data => { output += data; });
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const json = (route, body) => route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
const captureStatus = state => ({ state, source: state === 'idle' ? 'none' : 'live-capture', file: null, error: null, stopReason: null, library: null, adapter: null, report: null });
const lab = (overrides = {}) => ({
  server: 'https://albionlab.app', connected: true, account: { name: 'Tester', avatarUrl: '' }, pairing: null, pairingError: null,
  session: { id: 'live', url: 'https://albionlab.app/s/live', visibility: 'unlisted', anonymize: false, state: 'live', error: null, publishedAt: null, title: null },
  shareLocation: false, visibility: 'unlisted', anonymize: false, suggestedTitle: null, ...overrides
});
let browser;
const results = [];
/**
 * The user's action, as the routes see it: a request sent before it is held until the action has been answered and
 * then answered with what it saw, so it always arrives late. settled() waits until such a late answer was delivered.
 */
function action() {
  let done = false, open, late = 0;
  const answered = new Promise(resolve => { open = resolve; });
  return {
    get done() { return done; },
    finish: () => { done = true; setTimeout(open, 100); },
    hold: async () => { if (done) return false; await answered; late++; return true; },
    settled: async () => { for (let i = 0; i < 500 && late === 0; i++) await sleep(10); assert.ok(late > 0, 'no request sent before the action was answered after it'); await sleep(300); }
  };
}
async function check(name, body) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, locale: 'en-US' });
  const page = await context.newPage();
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await body(page, context);
    assert.deepEqual(errors, []);
    results.push(`PASS ${name}`); console.log(`PASS ${name}`);
  } catch (error) {
    results.push(`FAIL ${name}`); console.log(`FAIL ${name}: ${error.message}`);
    process.exitCode = 1;
  } finally { await context.close(); }
}
try {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server.exitCode !== null) throw new Error(output);
    try { if ((await fetch(`${base}/api/health`)).ok) break; } catch { }
    await sleep(100);
  }
  browser = await chromium.launch();

  await check('A capture status poll answered after Start does not re-enable Start', async page => {
    let state = 'idle';
    const start = action();
    await page.route('**/api/capture/devices', route => json(route, { available: true, library: null, debugEnabled: false, selectedDevice: 'raw:any', methods: [{ id: 'socket', name: 'Socket', available: true }], devices: [{ name: 'raw:any', description: 'All interfaces', method: 'socket', flags: 0 }], error: null }));
    await page.route('**/api/capture/status', async route => {
      const answer = captureStatus(state);
      if (state === 'idle') await start.hold();
      await json(route, answer);
    });
    await page.route('**/api/capture/start', async route => { state = 'running'; await json(route, captureStatus('running')); start.finish(); });
    await page.goto(base);
    await page.getByRole('button', { name: 'Capture', exact: true }).click();
    const button = page.getByRole('button', { name: 'Start capture', exact: true });
    await page.waitForFunction(() => [...document.querySelectorAll('button')].some(item => item.textContent === 'Start capture' && !item.disabled));
    // A poll sent with the capture idle is held while Start is pressed and answers after Start did.
    const started = page.waitForResponse(response => response.url().endsWith('/api/capture/start'));
    await button.click();
    await started;
    assert.ok(await button.isDisabled());
    await start.settled();
    assert.ok(await button.isDisabled(), 'Start was re-enabled by a poll sent before it');
    assert.ok(await page.getByRole('button', { name: 'Stop', exact: true }).isEnabled());
  });

  await check('The capture notice polls one status at a time', async page => {
    let inFlight = 0, most = 0, polls = 0, first = true;
    await page.route('**/api/capture/status', async route => {
      polls++; inFlight++; most = Math.max(most, inFlight);
      const slow = first; first = false;
      // The server stalls past the 2 s poll interval with the old "failed", then answers "running" at once.
      await sleep(slow ? 4500 : 50);
      inFlight--;
      await json(route, slow ? { ...captureStatus('failed'), error: 'stalled' } : captureStatus('running'));
    });
    await page.goto(base);
    await sleep(7000);
    assert.ok(polls >= 2);
    assert.equal(most, 1, 'two status polls were in flight together');
    assert.equal(await page.locator('.capture-alert').count(), 0, 'a stale "not recording" banner is shown');
  });

  await check('An AlbionLab poll answered after a settings change does not flip the checkbox back', async page => {
    let state = lab();
    const change = action();
    let loaded = false;
    await page.route('**/api/albionlab', async route => {
      const answer = state;
      // The first load shows the card; every poll after it is held across the change.
      if (loaded) await change.hold();
      loaded = true;
      await json(route, answer);
    });
    await page.route('**/api/albionlab/session', async route => {
      const body = route.request().postDataJSON();
      state = lab({ anonymize: body.anonymize ?? state.anonymize, session: { ...state.session, anonymize: body.anonymize ?? state.session.anonymize } });
      await json(route, state);
      change.finish();
    });
    await page.goto(base);
    const hide = page.getByRole('checkbox', { name: "Hide other players' names" });
    await hide.waitFor();
    assert.equal(await hide.isChecked(), false);
    // Wait for the next 2 s poll to go out with the unchecked state, then change the setting while it is held.
    await page.waitForRequest(request => request.url().endsWith('/api/albionlab'));
    const changed = page.waitForResponse(response => response.url().endsWith('/api/albionlab/session'));
    await hide.click();
    await changed;
    assert.equal(await hide.isChecked(), true);
    await change.settled();
    assert.equal(await hide.isChecked(), true, 'a poll sent before the change unchecked it');
  });

  await check('Double-clicking Connect account asks for one pairing code and opens one approval page', async (page, context) => {
    let pairs = 0;
    const popups = [];
    context.on('page', popup => popups.push(popup));
    await context.route('https://albionlab.app/**', route => route.fulfill({ status: 200, contentType: 'text/html', body: '<title>approve</title>' }));
    await page.route('**/api/albionlab', route => json(route, lab({ connected: false, account: null, session: null })));
    await page.route('**/api/albionlab/connect', async route => {
      pairs++;
      await sleep(500);
      await json(route, lab({ connected: false, account: null, session: null, pairing: { code: 'ABCD', verifyUrl: 'https://albionlab.app/link?code=ABCD', expiresAt: new Date(Date.now() + 600_000).toISOString() } }));
    });
    await page.goto(base);
    const connect = page.getByRole('button', { name: 'Connect account', exact: true });
    await connect.waitFor();
    await connect.dblclick();
    await sleep(1500);
    assert.equal(pairs, 1, `${pairs} pairing requests`);
    assert.equal(popups.length, 1, `${popups.length} approval pages`);
  });

  await check('The first game data load answered after an import does not bring the old catalog back', async page => {
    const load = action();
    await page.route('**/api/catalog', async route => {
      await load.hold();
      await json(route, { mapCount: 1, weaponCount: 1, gameVersion: 'before-import', importedAt: null, maps: [], weapons: [] });
    });
    await page.route('**/api/catalog/import', async route => {
      await json(route, { mapCount: 512, weaponCount: 904, gameVersion: 'after-import', importedAt: new Date().toISOString(), maps: [], weapons: [] });
      load.finish();
    });
    const game = join(directory, 'game');
    await mkdir(join(game, 'GameData', 'cluster'), { recursive: true });
    await writeFile(join(game, 'GameData', 'cluster', 'world.bin'), 'world');
    await page.goto(base);
    // The view's first load is held until the import has been answered.
    const loading = page.waitForRequest(request => request.url().endsWith('/api/catalog'));
    await page.getByRole('button', { name: 'Game data', exact: true }).click();
    await loading;
    await page.locator('input[type=file][webkitdirectory]').setInputFiles(game);
    await page.locator('dd', { hasText: 'after-import' }).waitFor();
    await load.settled();
    assert.equal(await page.locator('dd', { hasText: /-import$/ }).textContent(), 'after-import', 'the import was replaced by the first load');
  });

  await check('A double click on Play, Pause or Resume sends one command, and End session asks first', async page => {
    const initial = await (await fetch(`${base}/api/session`)).json();
    const startedAt = new Date(Date.now() + 1000).toISOString();
    let sequence = 0;
    const commands = [];
    const record = status => ({ ...initial, id: 'record', mode: 'live', status, startedAt, sequence: ++sequence, full: true });
    await page.route('**/api/session/*', route => {
      const action = route.request().url().split('/').pop();
      commands.push(action);
      return json(route, record(action === 'pause-record' ? 'paused' : action === 'stop-record' ? 'completed' : 'running'));
    });
    await page.route('**/api/capture/status', route => json(route, captureStatus('running')));
    await page.goto(base);
    // A real double click: the second click comes after the first command answered and its button was replaced.
    async function doubleClick(name) {
      const button = page.getByRole('button', { name, exact: true });
      await button.waitFor();
      await page.waitForFunction(label => [...document.querySelectorAll('button')].some(item => item.textContent === label && !item.disabled), name);
      const box = await button.boundingBox();
      // Far enough from the last double click, in time and place, that the browser starts a new click count.
      await page.mouse.move(0, 0); await sleep(700);
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down({ clickCount: 1 }); await page.mouse.up({ clickCount: 1 });
      await sleep(150);
      await page.mouse.down({ clickCount: 2 }); await page.mouse.up({ clickCount: 2 });
      await sleep(300);
      // Open issue (reported, not a command): after Play the End session button sits where Play was, so the second click
      // of a double click on Play opens the End confirmation. Nothing is sent; it is closed here to go on.
      if (await page.getByRole('dialog').count()) {
        console.log(`NOTE the second click of a double click on ${name} opened "${await page.getByRole('dialog').locator('h3').textContent()}"`);
        await page.keyboard.press('Escape');
        await page.getByRole('dialog').waitFor({ state: 'detached' });
      }
    }
    await doubleClick('Play');
    assert.deepEqual(commands, ['start-record']);
    await page.getByRole('button', { name: 'Pause', exact: true }).waitFor();
    await doubleClick('Pause');
    assert.deepEqual(commands, ['start-record', 'pause-record']);
    await page.getByRole('button', { name: 'Resume', exact: true }).waitFor();
    await doubleClick('Resume');
    assert.deepEqual(commands, ['start-record', 'pause-record', 'resume-record']);
    await page.getByRole('button', { name: 'Pause', exact: true }).waitFor();
    // A single click and the keyboard still work.
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await page.getByRole('button', { name: 'Resume', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Resume', exact: true }).focus();
    await page.keyboard.press('Enter');
    await page.getByRole('button', { name: 'Pause', exact: true }).waitFor();
    assert.deepEqual(commands, ['start-record', 'pause-record', 'resume-record', 'pause-record', 'resume-record']);
    // End session asks first (the same selectors record-controls.mjs uses): Cancel sends nothing, the modal's button ends it.
    await page.getByRole('button', { name: 'End session', exact: true }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.getByRole('dialog').waitFor({ state: 'detached' });
    assert.equal(commands.length, 5);
    await page.getByRole('button', { name: 'End session', exact: true }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'End session', exact: true }).click();
    await page.getByRole('button', { name: 'Play', exact: true }).waitFor();
    assert.equal(commands.at(-1), 'stop-record');
  });
} finally {
  if (browser) await browser.close();
  if (server.exitCode === null) { server.kill('SIGTERM'); await new Promise(resolve => server.once('exit', resolve)); }
  await rm(directory, { recursive: true, force: true });
}
console.log(`${results.filter(line => line.startsWith('PASS')).length}/${results.length} race checks passed.`);
