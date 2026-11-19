const fs = require('node:fs');
const greet = require('../../packages/greeting/dist/index.js');
fs.mkdirSync('dist', { recursive: true });
fs.writeFileSync('dist/message.txt', greet('Turborepo') + '\n');
