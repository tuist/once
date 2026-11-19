#!/usr/bin/env node
// Fails when any given file or directory contains a FIXME marker.
const fs = require('node:fs');
const path = require('node:path');
function check(target) {
  if (fs.statSync(target).isDirectory()) {
    return fs.readdirSync(target).every(entry => check(path.join(target, entry)));
  }
  if (!fs.readFileSync(target, 'utf8').includes('FIXME')) return true;
  console.error(`${target}: FIXME`);
  return false;
}
if (!process.argv.slice(2).every(check)) process.exit(1);
