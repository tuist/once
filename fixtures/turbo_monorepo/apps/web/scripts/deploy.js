const fs = require('node:fs');
console.log(`deploying ${fs.readFileSync('build/index.html', 'utf8').trim()}`);
