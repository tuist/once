const fs = require('node:fs');
fs.mkdirSync('dist', { recursive: true });
fs.writeFileSync('dist/index.js', "module.exports = name => 'Hello, ' + name + '!';\n");
