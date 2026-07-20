#!/bin/sh
# once input "../package.json"
# once input "../package-lock.json"
# once output "../node_modules"
# once cwd ".."
# once remote "remote_tests"
set -eu
npm clean-install --no-audit --no-fund
