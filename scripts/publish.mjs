import { spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const runtime = process.argv[2];
if (!['win-x64', 'linux-x64'].includes(runtime)) throw new Error('Usage: node scripts/publish.mjs win-x64|linux-x64');
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const output = `artifacts/${runtime}`;
const archive = `artifacts/AlbionMeter-0.1.0-${runtime}${runtime === 'win-x64' ? '-portable.zip' : '.tar.gz'}`;
const windows = runtime === 'win-x64';
const readme = [
  'Albion Meter 0.1.0' + (windows ? ' - Windows x64 portable' : ' - Linux x64'),
  '',
  windows ? 'Extract the entire ZIP, then open AlbionMeter.Server.exe. Accept the Windows administrator prompt.' : 'Run ./AlbionMeter.Server in a terminal.',
  'Open http://127.0.0.1:5187/ in your browser. Keep the console open while using the meter.',
  windows ? '.NET is included. No .NET installation or Npcap installation is required for socket capture.' : '.NET is included. Socket capture does not require libpcap.',
  '',
  'Recording controls:',
  '- Recording and network capture start stopped. Press Play in Overview to create a new encounter.',
  '- Pause freezes metrics; Resume continues the same encounter.',
  '- Stop saves the encounter and shuts down capture. The next Play starts from zero.',
  '- Starting an interface in Data only observes context and does not start recording combat.',
  '',
  'Capture methods (Data > Capture method):',
  windows
    ? '- System socket (Windows) is the default and runs as administrator.\n- The first socket capture creates a Windows Firewall rule for this executable.\n- If Npcap is installed, choose Npcap and its interface. Otherwise, use the system socket.'
    : '- System socket (Linux) is the default. Grant permission once:\n  sudo setcap cap_net_raw=ep ./AlbionMeter.Server\n- If libpcap is installed, it can also be selected in Data.',
  '',
  windows ? 'Portable data: records and settings stay in the data folder beside the executable. Keep this folder when updating the application.' : 'Records and settings stay in the local AlbionMeter user data folder.',
  'Normal mode saves processed combat/player/equipment data without recording raw PCAP files.',
  'Optional packet debugging: set Capture__Debug=true before starting. The packets table is limited to 10,000 packets / 32 MB of payloads.',
  'Leave Capture__Debug unset for normal use.',
  `Custom storage: ${windows ? 'AlbionMeter.Server.exe' : './AlbionMeter.Server'} --Storage:Directory=<folder>`,
  'Ctrl+C in the console shuts down the server. --open opens the browser at launch.',
  ''
].join('\n').replace(/\n/g, windows ? '\r\n' : '\n');
const commands = [
  [npm, ['run', 'build', '--prefix', 'web']],
  ['dotnet', ['publish', 'src/AlbionMeter.Server', '-c', 'Release', '-r', runtime,
    '--self-contained', 'true', '-o', output]]
];
for (const [command, args] of commands) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' && command === npm });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
writeFileSync(`${root}${output}/LEIA-ME.txt`, readme);
if (windows) { mkdirSync(`${root}${output}/data`, { recursive: true }); writeFileSync(`${root}${output}/data/.portable`, 'Albion Meter keeps its data in this folder.\r\n'); }
rmSync(`${root}${archive}`, { force: true });
const packed = spawnSync(windows && process.platform !== 'win32' ? 'bsdtar' : 'tar', windows ? ['-a', '-c', '-f', `../${archive.split('/')[1]}`, '.'] : ['-czf', `../${archive.split('/')[1]}`, '.'],
  { cwd: `${root}${output}`, stdio: 'inherit' });
if (packed.error || packed.status !== 0) { console.error('Packaging failed; the published folder is still available.'); process.exit(1); }
console.log(`Package: ${archive}`);
