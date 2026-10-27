import { build } from 'esbuild';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
const output = fileURLToPath(new URL('../src/AlbionMeter.Server/wwwroot/', import.meta.url));
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await build({
  absWorkingDir: root,
  entryPoints: ['src/main.tsx'],
  outdir: output,
  bundle: true,
  minify: true,
  format: 'esm',
  target: ['es2022'],
  jsx: 'automatic',
  legalComments: 'none',
  define: { 'process.env.NODE_ENV': '"production"' },
  logLevel: 'info'
});
const html = (await readFile(new URL('index.html', import.meta.url), 'utf8'))
  .replace('/src/main.tsx', '/main.js').replace('</head>', '<link rel="stylesheet" href="/main.css" /></head>');
await writeFile(`${output}/index.html`, html);
await cp(new URL('public/', import.meta.url), output, { recursive: true });
