#!/usr/bin/env bash
# struo needs "./generate.sh"
# struo input "../main.txt"
# struo output "../dist/app.txt"
# struo cwd ".."
set -e
mkdir -p dist
cat build/generated.txt main.txt > dist/app.txt
cat dist/app.txt
