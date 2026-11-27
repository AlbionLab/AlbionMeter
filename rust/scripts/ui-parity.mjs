// The UI parity gate: every screen of the fixture rendered by the React build (Playwright, every /api route and the
// SignalR stream answered from rust/crates/meter-app/fixtures/parity.json) and by the Slint app
// (`meter-app --screen <name> --screenshot <png>`, the same JSON), side by side in rust/target/ui-parity/.
// Journal screens use rust/scripts/parity-journal.tsx (the real DungeonJournal and HistoryTransfer, which main.tsx does
// not mount); `ranking-image*` screens compare the picture CopyDamage puts on the clipboard.
//
//   npm run build --prefix web        # once, or pass --build
//   node rust/scripts/ui-parity.mjs [--build] [screen …]
import { spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const web = createRequire(join(root, 'web/package.json'));
const { chromium } = web('playwright');
const fixturePath = join(root, 'rust/crates/meter-app/fixtures/parity.json');
const output = join(root, 'rust/target/ui-parity');
const site = join(root, 'src/AlbionMeter.Server/wwwroot');
const args = process.argv.slice(2);
const base = JSON.parse(await readFile(fixturePath, 'utf8'));
const names = args.filter(arg => !arg.startsWith('--'));
const screens = names.length ? names : Object.keys(base.screens);
// Settings written by the app go to a temporary folder, never to the real one.
const config = await mkdtemp(join(tmpdir(), 'albion-meter-parity-'));

if (args.includes('--build')) run('npm', ['run', 'build', '--prefix', join(root, 'web')]);
run('cargo', ['build', '-q', '-p', 'meter-app', '--features', 'parity'], join(root, 'rust'));
await mkdir(output, { recursive: true });

// The journal page: parity-journal.tsx bundled with the web app's own esbuild and node_modules.
const journalSite = join(config, 'journal');
await web('esbuild').build({
  entryPoints: [join(root, 'rust/scripts/parity-journal.tsx')], outdir: journalSite, bundle: true, format: 'esm', jsx: 'automatic',
  nodePaths: [join(root, 'web/node_modules')], define: { 'process.env.NODE_ENV': '"production"' }, logLevel: 'error',
});
await writeFile(join(journalSite, 'index.html'), (await readFile(join(site, 'index.html'), 'utf8')).replace('/main.js', '/parity-journal.js').replace('/main.css', '/parity-journal.css'));

function run(command, list, cwd = root) {
  const result = spawnSync(command, list, { cwd, encoding: 'utf8', env: { ...process.env, ALBION_METER_CONFIG_DIR: config } });
  if (result.status !== 0) throw new Error(`${command} ${list.join(' ')} failed:\n${result.stdout}${result.stderr}`);
}

/** Objects merged key by key, anything else replaced: the same rule as fixture.rs. */
function merge(target, over) {
  if (!over || typeof over !== 'object' || Array.isArray(over) || !target || typeof target !== 'object' || Array.isArray(target)) return structuredClone(over);
  const result = structuredClone(target);
  for (const [key, value] of Object.entries(over)) result[key] = key in result ? merge(result[key], value) : structuredClone(value);
  return result;
}

const types = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.svg': 'image/svg+xml', '.png': 'image/png' };
const json = (route, body, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
const icon = async (route, id, data) => {
  const file = data.icons[id];
  return file ? route.fulfill({ status: 200, contentType: 'image/png', body: await readFile(join(fixturePath, '..', file)) }) : route.fulfill({ status: 404 });
};

async function react(browser, name, data) {
  const ui = data.ui ?? {};
  const journal = ui.view === 'journal';
  // localhost is a secure context, so the Clipboard API (the ranking picture) is available.
  const context = await browser.newContext({ viewport: { width: data.window.width, height: data.window.height }, deviceScaleFactor: 1, locale: 'en-US', timezoneId: 'UTC' });
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: 'http://localhost' });
  const page = await context.newPage();
  // The Electron preload (desktop/preload.js), so the window controls, the update toast and the saved language show.
  await page.addInitScript(({ version, update, language }) => {
    window.albionDesktop = {
      info: async () => ({ version, platform: 'linux', maximized: false, update, language }),
      window: () => {}, onWindowState: () => () => {}, onUpdate: () => () => {}, update: () => {},
      permission: async () => false, language: async () => language, setLanguage: async () => {},
    };
  }, { version: data.health.version, update: data.update, language: ui.language ?? 'en' });
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    if (path.startsWith('/stream')) return stream(route, data, url);
    if (path === '/api/health') return json(route, data.health);
    if (path === '/api/albionlab') return json(route, data.albionlab);
    if (path === '/api/capture/status') return json(route, data.captureStatus);
    if (path === '/api/capture/devices') return json(route, data.devices);
    if (path === '/api/catalog') return json(route, data.catalog);
    if (path === '/api/storage') return json(route, data.storage);
    if (path === '/api/dungeons') return json(route, data.dungeons ?? []);
    if (path.startsWith('/api/dungeons/')) return data.dungeon ? json(route, data.dungeon) : json(route, {}, 404);
    if (path.startsWith('/api/items/')) { const item = data.items?.[path.slice('/api/items/'.length)]; return item ? json(route, item) : json(route, {}, 404); }
    if (path.startsWith('/api/weapon-icon/')) return icon(route, decodeURIComponent(path.slice('/api/weapon-icon/'.length)), data);
    if (path.startsWith('/api/item-icon/')) return icon(route, decodeURIComponent(path.slice('/api/item-icon/'.length)), data);
    if (path === '/api/history/import') return json(route, data.historyResult ?? {});
    if (path.startsWith('/api/session/')) return data.commandError ? json(route, { error: data.commandError }, 409) : json(route, data.snapshot);
    if (path.startsWith('/api/')) return json(route, {});
    const folder = journal && /^\/(index\.html|parity-journal\.(js|css))?$/.test(path) ? journalSite : site;
    const file = join(folder, path === '/' ? 'index.html' : path);
    return route.fulfill({ status: 200, contentType: types[extname(file)] ?? 'application/octet-stream', body: await readFile(file) });
  });
  await page.goto('http://localhost/');
  if (journal) await page.waitForSelector(data.dungeon ? '.run-hero' : '.large-empty');
  else if (data.connected) await page.waitForSelector('.mini-controls');
  else await page.waitForSelector('.app-loading');
  if (ui.card) return card(page, context, name, ui);
  if (journal) await journalActions(page, ui, data);
  else await meterActions(page, ui);
  // Icons, the AlbionLab and capture polls and fonts settle.
  await page.waitForTimeout(900);
  if (ui.scroll) await page.evaluate(y => { document.querySelector('.mini-main').scrollTop = y; }, ui.scroll);
  if (ui.hover) await page.getByRole('button', { name: ui.hover, exact: true }).first().hover();
  else await page.mouse.move(0, data.window.height - 1);
  await page.waitForTimeout(200);
  await page.screenshot({ path: join(output, `${name}-react.png`) });
  await context.close();
}

async function meterActions(page, ui) {
  const views = { capture: 1, game: 2 };
  if (views[ui.view]) await page.locator('.mini-bar nav button').nth(views[ui.view]).click();
  if (ui.metric === 'healing') await page.locator('.mini-bars .segmented button').nth(1).click();
  if (ui.confirm === 'reset') await page.locator('.mini-controls .button.ghost').click();
  if (ui.confirm === 'end') await page.locator('.mini-controls .button.danger').click();
  if (ui.action === 'play') { await page.locator('.mini-controls .button.go').click(); await page.waitForSelector('.error-banner'); }
  if (ui.copyMenu) await page.locator('.copy-trigger').click();
  if (ui.tab === 'maps') await page.locator('.catalog-panel .segmented button').nth(1).click();
  if (ui.search) await page.locator('.catalog-panel input.search').fill(ui.search);
}

async function journalActions(page, ui, data) {
  if (!data.dungeon) return;
  if (ui.floor) await page.getByRole('tab', { name: `Floor ${ui.floor}` }).click();
  if (ui.metric === 'healing') await page.locator('.chart-panel .heading-actions .segmented button').nth(1).click();
  if (ui.grouping === 'item') await page.locator('select[aria-label="Dungeon timeline grouping"]').selectOption('item');
  if (ui.range) await page.locator('select[aria-label="Dungeon timeline range"]').selectOption(ui.range);
  if (ui.key) { await page.evaluate(() => document.querySelector('.timeline-canvas canvas').focus({ preventScroll: true })); await page.keyboard.press(ui.key); }
  if (ui.history === 'import') {
    const file = join(config, 'history.zip');
    await writeFile(file, 'PK');
    await page.locator('input[aria-label="Import history file"]').setInputFiles(file);
    await page.waitForSelector('.history-transfer-result');
  }
}

/** The picture CopyDamage puts on the clipboard, read back as PNG. */
async function card(page, context, name, ui) {
  if (ui.card === 'healing') await page.locator('.mini-bars .segmented button').nth(1).click();
  await page.waitForTimeout(600);
  await page.locator('.copy-trigger').click();
  await page.getByRole('menuitem').first().click();
  await page.waitForSelector('.copy-trigger.copied', { timeout: 15000 });
  const png = await page.evaluate(async () => {
    const [item] = await navigator.clipboard.read();
    const bytes = new Uint8Array(await (await item.getType('image/png')).arrayBuffer());
    let text = ''; for (const byte of bytes) text += String.fromCharCode(byte);
    return btoa(text);
  });
  await writeFile(join(output, `${name}-react.png`), Buffer.from(png, 'base64'));
  await context.close();
}

/** The SignalR hub over long polling: negotiate, the handshake answer, one Snapshot invocation, then a poll left open. */
const polls = new WeakMap();
async function stream(route, data, url) {
  const request = route.request();
  if (!data.connected) return route.fulfill({ status: 503, body: '' });
  if (url.pathname.endsWith('/negotiate')) return json(route, { negotiateVersion: 1, connectionId: 'c', connectionToken: 't', availableTransports: [{ transport: 'LongPolling', transferFormats: ['Text', 'Binary'] }] });
  if (request.method() === 'POST') return route.fulfill({ status: 200, body: '' });
  if (request.method() === 'DELETE') return route.fulfill({ status: 202, body: '' });
  const page = request.frame().page();
  const count = (polls.get(page) ?? 0) + 1;
  polls.set(page, count);
  if (count === 1) return route.fulfill({ status: 200, body: '' });
  if (count === 2) {
    await new Promise(resolve => setTimeout(resolve, 150));
    const frames = ['{}', ...(data.snapshot ? [JSON.stringify({ type: 1, target: 'Snapshot', arguments: [data.snapshot] })] : [])];
    return route.fulfill({ status: 200, contentType: 'text/plain', body: frames.map(frame => `${frame}\u001e`).join('') });
  }
  // Held: the hub stays connected and quiet while the screenshot is taken.
  await new Promise(resolve => setTimeout(resolve, 60_000));
  return route.fulfill({ status: 200, body: '' }).catch(() => {});
}

function slint(name) {
  run('cargo', ['run', '-q', '-p', 'meter-app', '--features', 'parity', '--', '--fake', '--screen', name, '--screenshot', join(output, `${name}-slint.png`)], join(root, 'rust'));
}

const browser = await chromium.launch();
const failed = [];
try {
  for (const name of screens) {
    const data = merge(base, base.screens[name]);
    delete data.screens;
    try {
      await react(browser, name, data);
      slint(name);
      console.log(`${name}: ${join(output, name)}-{react,slint}.png`);
    } catch (error) {
      failed.push(name);
      console.log(`${name}: FAILED ${error.message}`);
    }
  }
} finally {
  await browser.close();
}
if (failed.length) { console.log(`failed: ${failed.join(', ')}`); process.exitCode = 1; }
