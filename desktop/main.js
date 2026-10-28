const { app, BrowserWindow, Menu, dialog, shell } = require('electron');
const { autoUpdater } = require('electron-updater');
const { spawn, execFile } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');

const origin = 'http://127.0.0.1:5187';
const checkEvery = 6 * 60 * 60 * 1000;
let window = null;
let server = null;
let stopping = null;
let leaving = false;

function log(message) {
  try {
    const folder = path.join(app.getPath('userData'), 'logs');
    fs.mkdirSync(folder, { recursive: true });
    fs.appendFileSync(path.join(folder, 'desktop.log'), `${new Date().toISOString()} ${message}\n`);
  } catch {}
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
  const capabilities = (await run('getcap', [executable])) ?? '';
  if (!capabilities.includes('cap_net_raw')) {
    const granted = await run('pkexec', ['setcap', 'cap_net_raw=ep', executable]);
    if (granted === null) {
      log('packet capture permission was not granted');
      await dialog.showMessageBox({ type: 'warning', title: 'Albion Meter', message: 'Packet capture permission was not granted.', detail: `Albion Meter opens, but it cannot read the game traffic. Allow it once with:\n\nsudo setcap cap_net_raw=ep "${executable}"` });
    }
  }
  return { folder, executable };
}

async function startServer() {
  if (await healthy()) return;
  const { folder, executable } = process.platform === 'linux' && app.isPackaged ? await linuxServer() : bundledServer();
  const logs = path.join(app.getPath('userData'), 'logs');
  fs.mkdirSync(logs, { recursive: true });
  const output = fs.openSync(path.join(logs, 'server.log'), 'a');
  server = spawn(executable, [], { cwd: folder, windowsHide: true, stdio: ['ignore', output, output], env: { ...process.env, ALBION_METER_DESKTOP: '1' } });
  server.on('exit', code => {
    server = null;
    log(`server exited with ${code}`);
    if (!leaving) {
      dialog.showErrorBox('Albion Meter', `The meter stopped unexpectedly (code ${code}). Details are in ${path.join(logs, 'server.log')}.`);
      app.quit();
    }
  });
  for (let attempt = 0; attempt < 120; attempt++) {
    if (await healthy()) return;
    if (!server) throw new Error('The meter did not start.');
    await wait(250);
  }
  throw new Error('The meter did not answer in 30 seconds.');
}

function stopServer() {
  if (!server) return Promise.resolve();
  stopping ??= (async () => {
    const running = server;
    const exited = new Promise(resolve => running.once('exit', resolve));
    await request('POST', '/api/shutdown');
    if (await Promise.race([exited.then(() => true), wait(10000).then(() => false)]) === false) running.kill();
  })();
  return stopping;
}

function external(url) {
  if (!url.startsWith(origin)) shell.openExternal(url);
}

function createWindow() {
  window = new BrowserWindow({
    width: 1360,
    height: 860,
    minWidth: 900,
    minHeight: 600,
    show: false,
    title: 'Albion Meter',
    backgroundColor: '#0a0d13',
    icon: path.join(__dirname, 'build', process.platform === 'win32' ? 'icon.ico' : 'icon.png'),
    webPreferences: { contextIsolation: true, sandbox: true },
  });
  window.once('ready-to-show', () => window.show());
  window.webContents.setWindowOpenHandler(({ url }) => { external(url); return { action: 'deny' }; });
  window.webContents.on('will-navigate', (event, url) => { if (!url.startsWith(origin) && !url.startsWith('file:')) { event.preventDefault(); external(url); } });
  window.on('closed', () => { window = null; });
  return window.loadFile(path.join(__dirname, 'loading.html'));
}

function watchUpdates() {
  if (!app.isPackaged) return;
  autoUpdater.autoDownload = true;
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.on('error', error => log(`update failed: ${error?.message ?? error}`));
  autoUpdater.on('update-downloaded', async info => {
    const { response } = await dialog.showMessageBox(window ?? undefined, {
      type: 'info',
      buttons: ['Restart now', 'Later'],
      defaultId: 0,
      cancelId: 1,
      title: 'Albion Meter',
      message: `Albion Meter ${info.version} is ready to install.`,
      detail: 'Restarting stops the current record. If you choose Later, it installs when you close the meter.',
    });
    if (response !== 0) return;
    leaving = true;
    await stopServer();
    autoUpdater.quitAndInstall(false, true);
  });
  const check = () => autoUpdater.checkForUpdates().catch(error => log(`update check failed: ${error?.message ?? error}`));
  check();
  setInterval(check, checkEvery);
}

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
    try {
      await startServer();
      await window?.loadURL(`${origin}/`);
      watchUpdates();
    } catch (error) {
      log(error.message);
      dialog.showErrorBox('Albion Meter', `${error.message}\n\nDetails are in ${path.join(app.getPath('userData'), 'logs')}.`);
      leaving = true;
      await stopServer();
      app.quit();
    }
  });
  app.on('window-all-closed', () => app.quit());
  app.on('before-quit', event => {
    if (!server || stopping) return;
    event.preventDefault();
    leaving = true;
    stopServer().finally(() => app.quit());
  });
}
