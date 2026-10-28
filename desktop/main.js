const { app, BrowserWindow, Menu, ipcMain, screen, session, shell } = require('electron');
const { autoUpdater } = require('electron-updater');
const { spawn, execFile } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');

const origin = 'http://127.0.0.1:5187';
const checkEvery = 6 * 60 * 60 * 1000;
const splash = { width: 400, height: 300 };
const splashFailed = { width: 440, height: 400 };
const minimum = { width: 820, height: 560 };
const needsPermission = process.platform === 'linux' && app.isPackaged;
let window = null;
let server = null;
let stopping = null;
let leaving = false;
let booting = false;
let startup = { steps: [], error: null };
let update = { state: 'idle' };
let expanded = false;

const logFolder = () => path.join(app.getPath('userData'), 'logs');

function log(message) {
  try {
    fs.mkdirSync(logFolder(), { recursive: true });
    fs.appendFileSync(path.join(logFolder(), 'desktop.log'), `${new Date().toISOString()} ${message}\n`);
  } catch {}
}

function send(channel, value) {
  if (window && !window.isDestroyed()) window.webContents.send(channel, value);
}

function resetStartup() {
  startup = {
    error: null,
    steps: [
      { id: 'server', label: 'Starting the capture service', state: 'pending' },
      ...(needsPermission ? [{ id: 'permission', label: 'Allowing packet capture', state: 'pending' }] : []),
      { id: 'meter', label: 'Loading the meter', state: 'pending' },
    ],
  };
  send('desktop:startup', startup);
}

function step(id, state, detail = null) {
  startup = { ...startup, steps: startup.steps.map(item => item.id === id ? { ...item, state, detail } : item) };
  send('desktop:startup', startup);
}

function fail(message) {
  log(message);
  startup = { ...startup, error: message, steps: startup.steps.map(item => item.state === 'active' ? { ...item, state: 'error' } : item) };
  send('desktop:startup', startup);
  if (window && !expanded) animate(centered(splashFailed), 220);
}

const easeOut = progress => 1 - Math.pow(1 - progress, 3);

function animate(target, duration = 360) {
  if (!window || window.isDestroyed()) return Promise.resolve();
  const from = window.getBounds();
  const started = Date.now();
  return new Promise(resolve => {
    const tick = () => {
      if (!window || window.isDestroyed()) return resolve();
      const progress = Math.min(1, (Date.now() - started) / duration);
      const eased = easeOut(progress);
      const mix = key => Math.round(from[key] + (target[key] - from[key]) * eased);
      window.setBounds({ x: mix('x'), y: mix('y'), width: mix('width'), height: mix('height') });
      if (progress < 1) setTimeout(tick, 1000 / 60);
      else resolve();
    };
    tick();
  });
}

function workArea() {
  return (window ? screen.getDisplayMatching(window.getBounds()) : screen.getPrimaryDisplay()).workArea;
}

function centered(size) {
  const area = workArea();
  return { width: size.width, height: size.height, x: Math.round(area.x + (area.width - size.width) / 2), y: Math.round(area.y + (area.height - size.height) / 2) };
}

const placementFile = () => path.join(app.getPath('userData'), 'window.json');

function savedPlacement() {
  try {
    const saved = JSON.parse(fs.readFileSync(placementFile(), 'utf8'));
    const visible = screen.getAllDisplays().some(display => {
      const area = display.workArea;
      return saved.x < area.x + area.width - 80 && saved.x + saved.width > area.x + 80 && saved.y >= area.y - 10 && saved.y < area.y + area.height - 80;
    });
    if (visible && saved.width >= minimum.width && saved.height >= minimum.height) return saved;
  } catch {}
  return null;
}

function savePlacement() {
  if (!window || window.isDestroyed() || !expanded) return;
  try { fs.writeFileSync(placementFile(), JSON.stringify({ ...window.getNormalBounds(), maximized: window.isMaximized() })); } catch {}
}

function idealBounds() {
  const area = workArea();
  const saved = savedPlacement();
  const width = Math.min(saved?.width ?? Math.max(minimum.width, Math.min(1280, area.width - 120)), area.width);
  const height = Math.min(saved?.height ?? Math.max(minimum.height, Math.min(820, area.height - 100)), area.height);
  return { ...centered({ width, height }), maximized: Boolean(saved?.maximized) };
}

const onNiri = process.platform === 'linux' && /niri/i.test(process.env.XDG_CURRENT_DESKTOP ?? '');

async function centerOnNiri() {
  if (!onNiri) return;
  try {
    const own = JSON.parse((await run('niri', ['msg', '--json', 'windows'])) ?? '[]').find(item => item.pid === process.pid);
    if (own) await run('niri', ['msg', 'action', 'center-window', '--id', String(own.id)]);
  } catch {}
}

async function expand() {
  if (!window || expanded) return;
  const target = idealBounds();
  window.setResizable(true);
  window.setMaximizable(true);
  await animate({ x: target.x, y: target.y, width: target.width, height: target.height }, 380);
  window.setMinimumSize(minimum.width, minimum.height);
  expanded = true;
  await centerOnNiri();
  if (target.maximized) window.maximize();
  window.focus();
}

async function shrink(size) {
  if (!window || !expanded) return;
  expanded = false;
  if (window.isMaximized()) window.unmaximize();
  window.setMinimumSize(splash.width, splash.height);
  await animate(centered(size), 300);
  await centerOnNiri();
  window.setMaximizable(false);
  window.setResizable(false);
}

function setUpdate(value) {
  update = value;
  send('desktop:update', update);
}

function request(method, route, timeout = 2000) {
  return new Promise(resolve => {
    const call = http.request(`${origin}${route}`, { method, timeout, headers: { 'X-Albion-Meter': 'local' } }, response => {
      response.resume();
      resolve(response.statusCode ?? 0);
    });
    call.on('timeout', () => call.destroy());
    call.on('error', () => resolve(0));
    call.end();
  });
}

const healthy = async () => (await request('GET', '/api/health')) === 200;
const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const run = (file, args) => new Promise(resolve => execFile(file, args, (error, stdout) => resolve(error ? null : stdout)));

function bundledServer() {
  const folder = app.isPackaged ? path.join(process.resourcesPath, 'server') : path.join(__dirname, 'server');
  return { folder, executable: path.join(folder, process.platform === 'win32' ? 'AlbionMeter.Server.exe' : 'AlbionMeter.Server') };
}

async function linuxServer() {
  const bundled = bundledServer();
  const root = path.join(app.getPath('userData'), 'server');
  const folder = path.join(root, app.getVersion());
  const executable = path.join(folder, 'AlbionMeter.Server');
  if (!fs.existsSync(executable)) {
    fs.rmSync(folder, { recursive: true, force: true });
    fs.cpSync(bundled.folder, folder, { recursive: true });
    fs.chmodSync(executable, 0o755);
  }
  for (const entry of fs.readdirSync(root)) if (entry !== app.getVersion()) fs.rmSync(path.join(root, entry), { recursive: true, force: true });
  step('permission', 'active');
  const capabilities = (await run('getcap', [executable])) ?? '';
  if (capabilities.includes('cap_net_raw')) {
    step('permission', 'done');
  } else {
    step('permission', 'active', 'Type your password in the system prompt to let the meter read the game traffic.');
    const granted = await run('pkexec', ['setcap', 'cap_net_raw=ep', executable]);
    if (granted === null) {
      log('packet capture permission was not granted');
      step('permission', 'warning', `Not allowed: the meter opens but cannot read the game traffic. Allow it later with sudo setcap cap_net_raw=ep "${executable}"`);
    } else {
      step('permission', 'done');
    }
  }
  return { folder, executable };
}

async function startServer() {
  if (await healthy()) return;
  const { folder, executable } = needsPermission ? await linuxServer() : bundledServer();
  fs.mkdirSync(logFolder(), { recursive: true });
  const output = fs.openSync(path.join(logFolder(), 'server.log'), 'a');
  const child = spawn(executable, [], { cwd: folder, windowsHide: true, stdio: ['ignore', output, output], env: { ...process.env, ALBION_METER_DESKTOP: '1' } });
  server = child;
  child.on('exit', code => {
    if (server === child) server = null;
    log(`server exited with ${code}`);
    if (!leaving && !booting) crashed(code);
  });
  for (let attempt = 0; attempt < 120; attempt++) {
    if (await healthy()) return;
    if (server !== child) throw new Error('The capture service stopped while starting.');
    await wait(250);
  }
  throw new Error('The capture service did not answer in 30 seconds.');
}

function stopServer() {
  if (!server) return Promise.resolve();
  stopping ??= (async () => {
    const running = server;
    const exited = new Promise(resolve => running.once('exit', resolve));
    await request('POST', '/api/shutdown');
    if (await Promise.race([exited.then(() => true), wait(10000).then(() => false)]) === false) running.kill();
  })().finally(() => { stopping = null; });
  return stopping;
}

async function boot() {
  if (booting) return;
  booting = true;
  resetStartup();
  try {
    step('server', 'active');
    await startServer();
    step('server', 'done');
    step('meter', 'active');
    await session.defaultSession.clearCache();
    startup = { ...startup, ready: true, steps: startup.steps.map(item => item.id === 'meter' ? { ...item, state: 'done' } : item) };
    send('desktop:startup', startup);
    await wait(260);
    await expand();
    await window.loadURL(`${origin}/`);
    watchUpdates();
  } catch (error) {
    fail(error.message);
    await stopServer();
  } finally {
    booting = false;
  }
}

async function crashed(code) {
  if (!window || window.isDestroyed()) return app.quit();
  savePlacement();
  await window.loadFile(path.join(__dirname, 'loading.html'));
  await shrink(splashFailed);
  resetStartup();
  fail(`The capture service stopped unexpectedly (code ${code}).`);
}

function external(url) {
  if (!url.startsWith(origin)) shell.openExternal(url);
}

function createWindow() {
  window = new BrowserWindow({
    ...centered(splash),
    resizable: false,
    maximizable: false,
    show: false,
    frame: false,
    title: 'Albion Meter',
    backgroundColor: '#0a0d13',
    icon: path.join(__dirname, 'build', process.platform === 'win32' ? 'icon.ico' : 'icon.png'),
    webPreferences: { contextIsolation: true, sandbox: true, preload: path.join(__dirname, 'preload.js') },
  });
  window.once('ready-to-show', () => { window.show(); centerOnNiri(); });
  window.on('maximize', () => send('desktop:window-state', { maximized: true }));
  window.on('unmaximize', () => send('desktop:window-state', { maximized: false }));
  window.webContents.setWindowOpenHandler(({ url }) => { external(url); return { action: 'deny' }; });
  window.webContents.on('will-navigate', (event, url) => { if (!url.startsWith(origin) && !url.startsWith('file:')) { event.preventDefault(); external(url); } });
  window.on('close', savePlacement);
  window.on('closed', () => { window = null; });
  return window.loadFile(path.join(__dirname, 'loading.html'));
}

let watching = false;
function watchUpdates() {
  if (!app.isPackaged || watching) return;
  watching = true;
  autoUpdater.autoDownload = true;
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.on('checking-for-update', () => { if (update.state !== 'ready') setUpdate({ state: 'checking' }); });
  autoUpdater.on('update-not-available', () => setUpdate({ state: 'current' }));
  autoUpdater.on('update-available', info => setUpdate({ state: 'downloading', version: info.version, percent: 0 }));
  autoUpdater.on('download-progress', progress => setUpdate({ state: 'downloading', version: update.version, percent: Math.round(progress.percent), transferred: progress.transferred, total: progress.total }));
  autoUpdater.on('update-downloaded', info => setUpdate({ state: 'ready', version: info.version }));
  autoUpdater.on('error', error => {
    log(`update failed: ${error?.message ?? error}`);
    if (update.state !== 'ready') setUpdate({ state: 'error', message: 'The update could not be downloaded. It will be tried again later.' });
  });
  const check = () => autoUpdater.checkForUpdates().catch(error => log(`update check failed: ${error?.message ?? error}`));
  check();
  setInterval(check, checkEvery);
}

ipcMain.handle('desktop:info', () => ({ version: app.getVersion(), platform: process.platform, maximized: window?.isMaximized() ?? false, startup, update, logs: logFolder() }));

ipcMain.on('desktop:window', (_event, action) => {
  if (!window) return;
  if (action === 'minimize') window.minimize();
  else if (action === 'maximize') window.isMaximized() ? window.unmaximize() : window.maximize();
  else if (action === 'close') window.close();
});

ipcMain.on('desktop:startup', (_event, action) => {
  if (action === 'retry') { if (!expanded) animate(centered(splash), 200); boot(); }
  else if (action === 'logs') shell.openPath(logFolder());
  else if (action === 'quit') app.quit();
});

ipcMain.on('desktop:update', async (_event, action) => {
  if (action === 'check') autoUpdater.checkForUpdates().catch(error => log(`update check failed: ${error?.message ?? error}`));
  if (action !== 'install' || update.state !== 'ready') return;
  setUpdate({ ...update, state: 'installing' });
  leaving = true;
  await stopServer();
  autoUpdater.quitAndInstall(true, true);
});

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (!window) return;
    if (window.isMinimized()) window.restore();
    window.focus();
  });
  app.whenReady().then(async () => {
    Menu.setApplicationMenu(null);
    await createWindow();
    await boot();
  });
  app.on('window-all-closed', () => app.quit());
  app.on('before-quit', event => {
    if (!server) return;
    event.preventDefault();
    leaving = true;
    stopServer().finally(() => app.quit());
  });
}
