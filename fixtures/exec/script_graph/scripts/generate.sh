#!/usr/bin/env bash
# struo input "../source.txt"
# struo output "../build/generated.txt"
# struo cwd ".."
set -e
mkdir -p build
printf 'generated:%s' "$(cat source.txt)" > build/generated.txt
