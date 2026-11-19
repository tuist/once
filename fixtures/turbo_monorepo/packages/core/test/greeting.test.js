const assert = require('node:assert/strict');
const test = require('node:test');
const { greeting } = require('../src/index.js');

test('greets by name', () => {
  assert.equal(greeting('Acme'), 'Hello, Acme!');
});
