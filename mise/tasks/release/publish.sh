#!/usr/bin/env bash
#MISE description="Bump the fabrik mise pin and publish the GitHub release"
#USAGE flag "--version <version>" help="Version being released"
#USAGE flag "--notes <notes>" help="Path to the rendered release notes file"
#USAGE flag "--dist <dist>" help="Directory containing release artifacts"
set -euo pipefail

version=""
notes=""
dist=""
while (($# > 0)); do
  case "$1" in
    --version)
      version="${2}"
      shift 2
      ;;
    --notes)
      notes="${2}"
      shift 2
      ;;
    --dist)
      dist="${2}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "${version}" ]]; then
  echo "--version is required" >&2
  exit 1
fi
if [[ -z "${notes}" ]]; then
  echo "--notes is required" >&2
  exit 1
fi
if [[ -z "${dist}" ]]; then
  echo "--dist is required" >&2
  exit 1
fi

# Repoint the mise tool pin so anyone running `mise install` against the
# repo picks up the version we are about to publish.
matches="$(grep -cE '^"github:tuist/fabrik"' mise.toml || true)"
if [[ "${matches}" != "1" ]]; then
  echo "expected one fabrik pin in mise.toml, found ${matches}" >&2
  exit 1
fi
sed -i.bak -E "s|^(\"github:tuist/fabrik\"[[:space:]]*=[[:space:]]*\")[^\"]+(\")|\1${version}\2|" mise.toml
rm -f mise.toml.bak

if ! git diff --quiet -- mise.toml; then
  git config user.name "github-actions[bot]"
  git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
  git add mise.toml
  git commit -m "chore: bump fabrik mise pin to ${version}"
  git push origin HEAD:main
fi

target="$(git rev-parse HEAD)"

mapfile -t artifacts < <(find "${dist}" -type f -print)

gh release create "${version}" \
  --title "${version}" \
  --notes-file "${notes}" \
  --target "${target}" \
  "${artifacts[@]}"
