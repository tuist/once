#!/usr/bin/env node
// Copies src/*.js into dist/, prefixed with the banner from @acme/config,
// and records what it compiled the way tsc records a build info file.
const fs = require('node:fs');
const path = require('node:path');
const config = JSON.parse(fs.readFileSync(require.resolve('@acme/config/compiler.json', { paths: [process.cwd()] }), 'utf8'));
const files = fs.readdirSync('src').filter(file => file.endsWith('.js')).sort();
fs.mkdirSync('dist', { recursive: true });
for (const file of files) {
  const source = fs.readFileSync(path.join('src', file), 'utf8');
  fs.writeFileSync(path.join('dist', file), config.banner + '\n' + source);
}
fs.mkdirSync('.cache', { recursive: true });
fs.writeFileSync('.cache/build-info.json', JSON.stringify({ files }) + '\n');
