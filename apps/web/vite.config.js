import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { createHash } from 'node:crypto';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
let buildRoot, workerSource;

export default defineConfig({
  plugins: [svelte(), {
    name: 'offline-shell',
    configResolved(config) { buildRoot = path.resolve(config.root, config.build.outDir); workerSource = path.resolve(config.root, 'public/sw.js'); },
    closeBundle() {
      const root = buildRoot;
      const files = fs.readdirSync(root, { recursive: true }).filter((file) => fs.statSync(path.join(root, file)).isFile() && file !== 'sw.js' && !file.endsWith('.map') && !file.endsWith('.br'));
      const hash = createHash('sha256');
      for (const file of files.sort()) hash.update(file).update(fs.readFileSync(path.join(root, file)));
      // Sign-in needs the network, so its phone-number formatting (a sixth of
      // the shell) is fetched and kept on first use instead of at install.
      const offline = (file) => file !== 'index.html' && !/^assets[\\/]phone-/.test(file);
      const manifest = ['/', ...files.filter(offline).map((file) => '/' + file.replaceAll(path.sep, '/'))];
      const source = fs.readFileSync(workerSource, 'utf8').replace('__BUILD_ID__', hash.digest('hex').slice(0, 16)).replace('__PRECACHE__', JSON.stringify(manifest));
      fs.writeFileSync(path.join(root, 'sw.js'), source);
      // Hosting serves these Brotli copies to browsers that accept Brotli, so
      // text travels at the best compression rather than the edge's on-the-fly level.
      for (const file of [...files, 'sw.js'].filter((file) => /\.(?:html|js|css)$/.test(file))) {
        const raw = fs.readFileSync(path.join(root, file));
        fs.writeFileSync(path.join(root, file + '.br'), zlib.brotliCompressSync(raw, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: raw.length } }));
      }
    },
  }],
  build: {
    target: 'es2022', modulePreload: { polyfill: false },
    rolldownOptions: { output: { codeSplitting: { groups: [{ name: 'phone', test: /node_modules[\\/]libphonenumber-js[\\/]/ }] } } },
  },
  server: { proxy: { '/api': 'http://localhost:8080', '/catalog': 'http://localhost:8080', '/status': 'http://localhost:8080', '/admin': 'http://localhost:8080' } },
});
