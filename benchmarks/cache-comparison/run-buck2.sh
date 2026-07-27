#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"

cd "$root/buck2"
mise exec -- buck2 build //:distribution --console simple
