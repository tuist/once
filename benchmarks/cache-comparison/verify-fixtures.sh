#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"

for workspace in bazel buck2 struo; do
  cmp "$root/fixture/action.mjs" "$root/$workspace/fixture/action.mjs"
  for input in "$root"/inputs/*.txt; do
    name="${input##*/}"
    cmp "$input" "$root/$workspace/inputs/$name"
  done
done
