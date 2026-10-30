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
let splashWindow = null;
let server = null;
let executable = null;
let stopping = null;
let leaving = false;
let booting = false;
let startup = { steps: [], error: null };
let update = { state: 'idle' };

const logFolder = () => path.join(app.getPath('userData'), 'logs');

function log(message) {
  try {
    fs.mkdirSync(logFolder(), { recursive: true });
    fs.appendFileSync(path.join(logFolder(), 'desktop.log'), `${new Date().toISOString()} ${message}\n`);
  } catch {}
}

function send(channel, value) {
  for (const target of [splashWindow, window]) if (target && !target.isDestroyed()) target.webContents.send(channel, value);
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
  if (splashWindow && !splashWindow.isDestroyed()) splashWindow.setBounds(centered(splashFailed));
}

const easeOut = progress => 1 - Math.pow(1 - progress, 3);
const fades = process.platform === 'win32' || process.platform === 'darwin';

function fade(target, from, to, duration) {
  if (!fades || !target || target.isDestroyed()) return Promise.resolve();
  const started = Date.now();
  return new Promise(resolve => {
    const tick = () => {
      if (target.isDestroyed()) return resolve();
      const progress = Math.min(1, (Date.now() - started) / duration);
      target.setOpacity(from + (to - from) * easeOut(progress));
      if (progress < 1) setTimeout(tick, 1000 / 60);
      else resolve();
    };
    tick();
  });
}

function workArea() {
  const anchor = [splashWindow, window].find(item => item && !item.isDestroyed());
  return (anchor ? screen.getDisplayMatching(anchor.getBounds()) : screen.getPrimaryDisplay()).workArea;
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
  if (!window || window.isDestroyed() || !window.isVisible()) return;
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

async function centerOnNiri(target) {
  if (!onNiri || !target || target.isDestroyed()) return;
  try {
    const title = target.getTitle();
    const own = JSON.parse((await run('niri', ['msg', '--json', 'windows'])) ?? '[]').find(item => item.pid === process.pid && item.title === title);
    if (own) await run('niri', ['msg', 'action', 'center-window', '--id', String(own.id)]);
  } catch {}
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

/** The version of whatever capture service answers on the port, or null when none does. */
function serverVersion() {
  return new Promise(resolve => {
    const call = http.get(`${origin}/api/health`, { timeout: 2000 }, response => {
      let body = '';
      response.setEncoding('utf8');
      response.on('data', chunk => { body += chunk; });
      response.on('end', () => { try { resolve(JSON.parse(body).version ?? null); } catch { resolve(null); } });
    });
    call.on('timeout', () => call.destroy());
    call.on('error', () => resolve(null));
  });
}
const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const run = (file, args) => new Promise(resolve => execFile(file, args, (error, stdout) => resolve(error ? null : stdout)));

function bundledServer() {
  const folder = app.isPackaged ? path.join(process.resourcesPath, 'server') : path.join(__dirname, 'server');
  return { folder, executable: path.join(folder, process.platform === 'win32' ? 'AlbionMeter.Server.exe' : 'AlbionMeter.Server') };
}

async function linuxServer() {
  const bundled = bundledServer();
  if (((await run('getcap', [bundled.executable])) ?? '').includes('cap_net_raw')) return bundled;
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
  if (await healthy()) {
    // A service of this version that this app started is reused; anything else (an older version left running, a
    // copy started by hand) is asked to stop, so the app always talks to its own service and can restart it.
    if (!app.isPackaged || (server && await serverVersion() === app.getVersion())) return;
    log(`stopping a capture service this app did not start (version ${await serverVersion()})`);
    await request('POST', '/api/shutdown');
    for (let attempt = 0; attempt < 40 && await healthy(); attempt++) await wait(250);
    if (await healthy()) throw new Error('Another capture service is using port 5187 and did not stop.');
  }
  const { folder, executable: file } = needsPermission ? await linuxServer() : bundledServer();
  executable = file;
  fs.mkdirSync(logFolder(), { recursive: true });
  const output = fs.openSync(path.join(logFolder(), 'server.log'), 'a');
  const child = spawn(file, [], { cwd: folder, windowsHide: true, stdio: ['ignore', output, output], env: { ...process.env, ALBION_METER_DESKTOP: '1' } });
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
    if (await Promise.race([exited.then(() => true), wait(10000).then(() => false)]) === false) {
      running.kill();
      if (await Promise.race([exited.then(() => true), wait(5000).then(() => false)]) === false) running.kill('SIGKILL');
    }
    if (server === running) server = null;
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
    await createMain();
    startup = { ...startup, ready: true, steps: startup.steps.map(item => item.id === 'meter' ? { ...item, state: 'done' } : item) };
    send('desktop:startup', startup);
    await wait(180);
    await reveal();
    watchUpdates();
  } catch (error) {
    if (window && !window.isDestroyed()) window.destroy();
    fail(error.message);
    await stopServer();
  } finally {
    booting = false;
  }
}

const restarts = [];

async function crashed(code) {
  // A crash in the middle of a run restarts the service by itself (twice in ten minutes at most): the window stays and
  // reconnects, and the capture goes on without waiting for someone to notice.
  while (restarts.length && Date.now() - restarts[0] > 10 * 60 * 1000) restarts.shift();
  if (restarts.length < 2 && !booting) {
    restarts.push(Date.now());
    log(`restarting the capture service after code ${code}`);
    booting = true;
    try { await wait(1000); await startServer(); return; }
    catch (error) { log(`automatic restart failed: ${error.message}`); }
    finally { booting = false; }
  }
  savePlacement();
  if (!splashWindow) await createSplash();
  if (window && !window.isDestroyed()) window.destroy();
  resetStartup();
  fail(`The capture service stopped unexpectedly (code ${code}).`);
}

function external(url) {
  if (!url.startsWith(origin)) shell.openExternal(url);
}

function frameless(options) {
  const created = new BrowserWindow({
    ...options,
    show: false,
    frame: false,
    title: 'Albion Meter',
    backgroundColor: '#0a0d13',
    icon: path.join(__dirname, 'build', process.platform === 'win32' ? 'icon.ico' : 'icon.png'),
    webPreferences: { contextIsolation: true, sandbox: true, preload: path.join(__dirname, 'preload.js') },
  });
  created.webContents.setWindowOpenHandler(({ url }) => { external(url); return { action: 'deny' }; });
  created.webContents.on('will-navigate', (event, url) => { if (!url.startsWith(origin) && !url.startsWith('file:')) { event.preventDefault(); external(url); } });
  return created;
}

function createSplash() {
  splashWindow = frameless({ ...centered(splash), resizable: false, maximizable: false, fullscreenable: false });
  const created = splashWindow;
  created.once('ready-to-show', () => { created.show(); centerOnNiri(created); });
  created.on('closed', () => { if (splashWindow === created) splashWindow = null; });
  return created.loadFile(path.join(__dirname, 'loading.html'));
}

async function createMain() {
  const target = idealBounds();
  window = frameless({ x: target.x, y: target.y, width: target.width, height: target.height, minWidth: minimum.width, minHeight: minimum.height });
  const created = window;
  created.on('maximize', () => send('desktop:window-state', { maximized: true }));
  created.on('unmaximize', () => send('desktop:window-state', { maximized: false }));
  created.on('close', savePlacement);
  created.on('closed', () => { if (window === created) window = null; });
  const ready = new Promise(resolve => created.once('ready-to-show', resolve));
  await created.loadURL(`${origin}/`);
  await Promise.race([ready, wait(4000)]);
  if (target.maximized) created.maximize();
}

async function reveal() {
  const shown = window;
  if (!shown || shown.isDestroyed()) return;
  if (fades) shown.setOpacity(0);
  shown.show();
  shown.focus();
  centerOnNiri(shown);
  await Promise.all([fade(shown, 0, 1, 240), fade(splashWindow, 1, 0, 200)]);
  if (splashWindow && !splashWindow.isDestroyed()) splashWindow.destroy();
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
    if (update.state === 'installing') return installFailed();
    if (update.state !== 'ready') setUpdate({ state: 'error', message: 'The update could not be downloaded. It will be tried again later.' });
  });
  const check = () => autoUpdater.checkForUpdates().catch(error => log(`update check failed: ${error?.message ?? error}`));
  check();
  setInterval(check, checkEvery);
}

ipcMain.handle('desktop:info', event => ({ version: app.getVersion(), platform: process.platform, maximized: BrowserWindow.fromWebContents(event.sender)?.isMaximized() ?? false, startup, update, logs: logFolder() }));

ipcMain.on('desktop:window', (event, action) => {
  const target = BrowserWindow.fromWebContents(event.sender);
  if (!target) return;
  if (action === 'minimize') target.minimize();
  else if (action === 'maximize' && target.isMaximizable()) target.isMaximized() ? target.unmaximize() : target.maximize();
  else if (action === 'close') target.close();
});

ipcMain.on('desktop:startup', (_event, action) => {
  if (action === 'retry') { splashWindow?.setBounds(centered(splash)); boot(); }
  else if (action === 'logs') shell.openPath(logFolder());
  else if (action === 'quit') app.quit();
});

ipcMain.handle('desktop:permission', async () => {
  if (process.platform !== 'linux' || !executable || booting) return false;
  if ((await run('pkexec', ['setcap', 'cap_net_raw=ep', executable])) === null) return false;
  booting = true;
  try {
    await stopServer();
    await startServer();
    return true;
  } catch (error) {
    log(`restart after permission failed: ${error.message}`);
    crashed(null);
    return false;
  } finally {
    booting = false;
  }
});

ipcMain.on('desktop:update', async (_event, action) => {
  if (action === 'check') autoUpdater.checkForUpdates().catch(error => log(`update check failed: ${error?.message ?? error}`));
  if (action !== 'install' || update.state !== 'ready') return;
  setUpdate({ ...update, state: 'installing' });
  leaving = true;
  await stopServer();
  autoUpdater.quitAndInstall(true, true);
  // Still running long after asking to install (a cancelled password prompt, a package manager error): bring the
  // service back on this version instead of leaving the window without it.
  setTimeout(() => { if (update.state === 'installing') installFailed(); }, 20000);
});

async function installFailed() {
  log('the update was not installed; starting the service again');
  leaving = false;
  setUpdate({ state: 'error', message: 'The update could not be installed. The meter keeps running on this version.' });
  try { await startServer(); }
  catch (error) { log(`restart after a failed install: ${error.message}`); crashed(null); }
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => {
    const target = window ?? splashWindow;
    if (!target) return;
    if (target.isMinimized()) target.restore();
    target.focus();
  });
  app.whenReady().then(async () => {
    Menu.setApplicationMenu(null);
    await createSplash();
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
