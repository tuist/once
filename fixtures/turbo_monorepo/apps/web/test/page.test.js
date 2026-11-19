const assert = require('node:assert/strict');
const test = require('node:test');
const render = require('../src/page.js');

test('renders the greeting from @acme/core', () => {
  assert.equal(render(), '<h1>Hello, Turborepo!</h1>');
});
