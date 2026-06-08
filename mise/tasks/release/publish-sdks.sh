#!/usr/bin/env bash
#MISE description="Publish RubyGems and npm SDK packages"
#USAGE flag "--version <version>" help="Version being released"
set -euo pipefail

version=""
while (($# > 0)); do
  case "$1" in
    --version)
      version="${2}"
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

if [[ ! -d packages/js/prebuilds || ! -d packages/ruby/prebuilds ]]; then
  echo "SDK prebuilds are missing; run release:package-sdk-libs first" >&2
  exit 1
fi

stage_dir="$(mktemp -d)"
trap 'rm -rf "${stage_dir}"' EXIT

mkdir -p "${stage_dir}/js" "${stage_dir}/ruby"
cp -R packages/js/. "${stage_dir}/js/"
cp -R packages/ruby/. "${stage_dir}/ruby/"
rm -rf "${stage_dir}/js/node_modules" "${stage_dir}/ruby/"*.gem

(
  cd "${stage_dir}/js"
  npm version "${version}" \
    --workspace=false \
    --no-git-tag-version \
    --allow-same-version
  npm publish --access public
)

(
  cd "${stage_dir}/ruby"
  ONCE_VERSION="${version}" gem build once.gemspec
  gem push "once-cache-${version}.gem"
)
