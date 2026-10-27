import { spawn, spawnSync } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const logDirectory = fileURLToPath(new URL('../.dev/', import.meta.url));
await mkdir(logDirectory, { recursive: true });
const logPath = `${logDirectory}startup.log`;
const log = createWriteStream(logPath, { flags: 'w' });
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
let activeChild;
function run(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: ['inherit', 'pipe', 'pipe'],
      env: { ...process.env, DOTNET_CLI_DO_NOT_USE_MSBUILD_SERVER: '1' },
      shell: process.platform === 'win32' && command === npm
    });
    activeChild = child;
    child.stdout.on('data', data => { process.stdout.write(data); log.write(data); });
    child.stderr.on('data', data => { process.stderr.write(data); log.write(data); });
    child.once('error', reject);
    child.once('close', code => {
      activeChild = undefined;
      code === 0 ? resolve() : reject(new Error(`${command} exited with code ${code}. See ${logPath}`));
    });
  });
}
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => activeChild?.kill(signal));
try {
  console.log(`Startup log: ${logPath}`);
  await run(npm, ['run', 'build', '--prefix', 'web']);
  await run('dotnet', ['build', 'src/AlbionMeter.Server', '--disable-build-servers', '-m:1', '-p:BuildInParallel=false']);
  console.log('Starting the server. Wait for "Now listening on: http://127.0.0.1:5187" before opening the browser.');
  const executable = `${root}src/AlbionMeter.Server/bin/Debug/net10.0/AlbionMeter.Server${process.platform === 'win32' ? '.exe' : ''}`;
  if (process.platform === 'linux' && !/cap_net_raw/.test(spawnSync('getcap', [executable], { encoding: 'utf8' }).stdout ?? '')) {
    console.log('Packet capture permission is missing after the build. Authorize it to keep recording combat.');
    const granted = spawnSync('pkexec', ['/usr/bin/setcap', 'cap_net_raw=ep', executable], { stdio: 'inherit' });
    if (granted.status !== 0) console.warn(`Capture stays disabled until you run: sudo setcap cap_net_raw=ep ${executable}`);
  }
  await run(executable, ['--open']);
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
} finally {
  log.end();
}
