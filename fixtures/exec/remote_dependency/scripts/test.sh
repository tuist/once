#!/bin/sh
# struo needs "./install.sh"
# struo input "../src"
# struo input "../tests"
# struo output "../reports"
# struo cwd ".."
# struo remote "remote_tests"
set -eu
test ! -e undeclared.txt
./node_modules/.bin/vitest run --reporter=json --outputFile=reports/vitest.json >/dev/null
node -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync("reports/vitest.json", "utf8"));
  if (!report.success || report.numPassedTests !== 1) process.exit(1);
  process.stdout.write("source:test");
'
