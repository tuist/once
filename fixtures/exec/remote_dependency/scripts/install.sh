#!/bin/sh
# struo input "../package.json"
# struo input "../package-lock.json"
# struo output "../node_modules"
# struo cwd ".."
# struo remote "remote_tests"
set -eu
npm clean-install --no-audit --no-fund
