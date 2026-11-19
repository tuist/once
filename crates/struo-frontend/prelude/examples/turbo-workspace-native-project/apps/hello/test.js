const assert = require('node:assert/strict');
const fs = require('node:fs');
assert.equal(fs.readFileSync('dist/message.txt', 'utf8'), 'Hello, Turborepo!\n');
