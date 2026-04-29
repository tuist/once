#!/usr/bin/env bash
#MISE description="Regenerate CHANGELOG.md from conventional commits"
set -euo pipefail

git cliff --config cliff.toml --bump --output CHANGELOG.md
