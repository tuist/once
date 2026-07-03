#!/usr/bin/env bash
# once input "../source.txt"
# once output "../build/generated.txt"
# once cwd ".."
set -e
mkdir -p build
printf 'generated:%s' "$(cat source.txt)" > build/generated.txt
