#!/usr/bin/env bash
# once needs "./generate.sh"
# once input "../main.txt"
# once output "../dist/app.txt"
# once cwd ".."
set -e
mkdir -p dist
cat build/generated.txt main.txt > dist/app.txt
cat dist/app.txt
