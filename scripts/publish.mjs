import { spawnSync } from 'node:child_process';
import { rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const runtime = process.argv[2];
if (!['win-x64', 'linux-x64'].includes(runtime)) throw new Error('Usage: node scripts/publish.mjs win-x64|linux-x64');
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const output = `artifacts/${runtime}`;
const archive = `artifacts/AlbionMeter-0.1.0-${runtime}${runtime === 'win-x64' ? '.zip' : '.tar.gz'}`;
const windows = runtime === 'win-x64';
const executable = windows ? 'AlbionMeter.Server.exe' : './AlbionMeter.Server';
const readme = [
  `Albion Meter 0.1.0 - ${windows ? 'Windows x64' : 'Linux x64'}`,
  'https://albionlab.app - tutorial: https://albionlab.app/tutorial',
  '',
  '== English ==',
  '',
  'Requirements',
  ...(windows ? [
    '- Windows 10 or 11, 64-bit.',
    '- .NET Runtime 10 (x64): https://aka.ms/dotnet/10.0/dotnet-runtime-win-x64.exe',
    '- ASP.NET Core Runtime 10 (x64): https://aka.ms/dotnet/10.0/aspnetcore-runtime-win-x64.exe',
    '  All versions: https://dotnet.microsoft.com/download/dotnet/10.0',
    '- Optional: Npcap, only to use the Npcap capture method: https://npcap.com/#download',
  ] : [
    '- 64-bit Linux with the ASP.NET Core Runtime 10:',
    '  Ubuntu/Debian: sudo apt install aspnetcore-runtime-10.0',
    '  Fedora: sudo dnf install aspnetcore-runtime-10.0',
    '  Arch: sudo pacman -S aspnet-runtime',
    '  Other distributions: https://learn.microsoft.com/dotnet/core/install/linux',
    '- Optional: libpcap, only to use the libpcap capture method (libpcap0.8 on Ubuntu/Debian, libpcap elsewhere).',
  ]),
  '',
  'Start',
  ...(windows ? [
    '1. Extract the whole ZIP to a folder.',
    '2. Open AlbionMeter.Server.exe and accept the administrator prompt (packet capture needs it).',
  ] : [
    '1. Extract the archive and allow packet capture once:',
    '   sudo setcap cap_net_raw=ep ./AlbionMeter.Server',
    '2. Run ./AlbionMeter.Server in a terminal.',
  ]),
  '3. Open http://127.0.0.1:5187 in your browser. Keep the console window open while you play.',
  '4. Game data > Import game folder > choose the folder that contains Albion-Online_Data.',
  '5. Capture > keep System socket, choose your network interface > Start capture (every time you open the meter).',
  '6. Meter > Play, then fight. Pause/Resume, Stop and Reset control the record.',
  '7. Optional: Connect account in the AlbionLab card to publish each record as a live session on albionlab.app.',
  '',
  `Records and settings are stored in ${windows ? '%LOCALAPPDATA%\\AlbionMeter' : '~/.local/share/AlbionMeter'}. Another folder: ${executable} --Storage:Directory=<folder>`,
  'Ctrl+C in the console closes the meter. --open opens the browser at launch.',
  '',
  '== Português ==',
  '',
  'Requisitos',
  ...(windows ? [
    '- Windows 10 ou 11, 64 bits.',
    '- .NET Runtime 10 (x64): https://aka.ms/dotnet/10.0/dotnet-runtime-win-x64.exe',
    '- ASP.NET Core Runtime 10 (x64): https://aka.ms/dotnet/10.0/aspnetcore-runtime-win-x64.exe',
    '  Todas as versões: https://dotnet.microsoft.com/download/dotnet/10.0',
    '- Opcional: Npcap, só para usar o método de captura Npcap: https://npcap.com/#download',
  ] : [
    '- Linux 64 bits com o ASP.NET Core Runtime 10:',
    '  Ubuntu/Debian: sudo apt install aspnetcore-runtime-10.0',
    '  Fedora: sudo dnf install aspnetcore-runtime-10.0',
    '  Arch: sudo pacman -S aspnet-runtime',
    '  Outras distribuições: https://learn.microsoft.com/dotnet/core/install/linux',
    '- Opcional: libpcap, só para usar o método de captura libpcap (libpcap0.8 no Ubuntu/Debian, libpcap nas outras).',
  ]),
  '',
  'Como abrir',
  ...(windows ? [
    '1. Extraia o ZIP inteiro numa pasta.',
    '2. Abra o AlbionMeter.Server.exe e aceite o pedido de administrador (a captura precisa disso).',
  ] : [
    '1. Extraia o arquivo e libere a captura uma vez:',
    '   sudo setcap cap_net_raw=ep ./AlbionMeter.Server',
    '2. Execute ./AlbionMeter.Server num terminal.',
  ]),
  '3. Abra http://127.0.0.1:5187 no navegador. Deixe a janela do console aberta enquanto joga.',
  '4. Game data > Import game folder > escolha a pasta que contém Albion-Online_Data.',
  '5. Capture > deixe System socket, escolha sua placa de rede > Start capture (toda vez que abrir o meter).',
  '6. Meter > Play e lute. Pause/Resume, Stop e Reset controlam a gravação.',
  '7. Opcional: Connect account no card do AlbionLab para publicar cada gravação ao vivo no albionlab.app.',
  '',
  `Gravações e configurações ficam em ${windows ? '%LOCALAPPDATA%\\AlbionMeter' : '~/.local/share/AlbionMeter'}. Outra pasta: ${executable} --Storage:Directory=<pasta>`,
  'Ctrl+C no console fecha o meter. --open abre o navegador ao iniciar.',
  ''
].join('\n').replace(/\n/g, windows ? '\r\n' : '\n');
const commands = [
  [npm, ['run', 'build', '--prefix', 'web']],
  ['dotnet', ['publish', 'src/AlbionMeter.Server', '-c', 'Release', '-r', runtime,
    '--self-contained', 'false', '-p:DebugType=None', '-p:DebugSymbols=false', '-o', output]]
];
for (const [command, args] of commands) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' && command === npm });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
rmSync(`${root}${output}/web.config`, { force: true });
writeFileSync(`${root}${output}/LEIA-ME.txt`, readme);
rmSync(`${root}${archive}`, { force: true });
const packed = spawnSync(windows && process.platform !== 'win32' ? 'bsdtar' : 'tar', windows ? ['-a', '-c', '-f', `../${archive.split('/')[1]}`, '.'] : ['-czf', `../${archive.split('/')[1]}`, '.'],
  { cwd: `${root}${output}`, stdio: 'inherit' });
if (packed.error || packed.status !== 0) { console.error('Packaging failed; the published folder is still available.'); process.exit(1); }
console.log(`Package: ${archive}`);
