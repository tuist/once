// Renders the page into build/, like a framework build, and keeps a cache
// directory that is never restored.
const fs = require('node:fs');
const render = require('../src/page.js');
const mode = process.env.APP_MODE || 'development';
fs.mkdirSync('build/cache', { recursive: true });
fs.writeFileSync('build/index.html', `${render()}\n<!-- ${mode} -->\n`);
fs.writeFileSync('build/cache/timestamp', String(Date.now()));
