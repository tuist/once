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

version="${version//[[:space:]]/}"
if [[ -z "${version}" ]]; then
  echo "--version is required" >&2
  exit 1
fi

semver_pattern='^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-((0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*))?([+]([0-9A-Za-z-]+([.][0-9A-Za-z-]+)*))?$'
if [[ ! "${version}" =~ ${semver_pattern} ]]; then
  echo "--version must be a semantic version" >&2
  exit 1
fi

for tool in gem npm; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} is required" >&2
    exit 1
  fi
done

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

npm_has_version() {
  npm view "buildonce@${version}" version >/dev/null 2>&1
}

gem_has_version() {
  # `gem list --remote` prints `buildonce (0.53.1, 0.53.0, ...)`, so the
  # version list is unwrapped and matched whole to avoid 0.5.4 matching
  # 0.54.0. grep runs without -q so that it drains the pipeline instead of
  # closing it early and tripping pipefail on a SIGPIPE from tr.
  gem list --remote --exact --all buildonce 2>/dev/null |
    sed -n 's/^buildonce (\(.*\))$/\1/p' |
    tr ',' '\n' |
    sed 's/[[:space:]]//g' |
    grep -Fx "${version}" >/dev/null
}

publish_npm() {
  if npm_has_version; then
    echo "buildonce@${version} is already on npm; skipping"
    return 0
  fi

  # npm authenticates through OIDC trusted publishing. Clients older than
  # 11.5.1 do not know how to make that exchange, quietly fall back to an
  # anonymous request, and the registry answers it with a 404 claiming the
  # package does not exist. Checking here turns that into an error that
  # names the real problem.
  local npm_min_version="11.5.1"
  local npm_version
  npm_version="$(npm --version)"
  if [[ "$(printf '%s\n%s\n' "${npm_min_version}" "${npm_version}" | sort -V | head -n1)" != "${npm_min_version}" ]]; then
    echo "npm ${npm_min_version} or newer is required to publish; found ${npm_version}" >&2
    return 1
  fi

  (
    cd "${stage_dir}/js" &&
      npm version "${version}" --no-git-tag-version --allow-same-version &&
      npm publish --access public --loglevel verbose
  )
}

publish_gem() {
  if gem_has_version; then
    echo "buildonce ${version} is already on RubyGems; skipping"
    return 0
  fi

  (
    cd "${stage_dir}/ruby" &&
      ONCE_VERSION="${version}" gem build once.gemspec &&
      gem push "buildonce-${version}.gem"
  )
}

# The registries are published independently so an outage or an expired
# credential on one does not leave the other silently unreleased. Each step is
# a no-op once the version is on the registry, which makes re-running the job
# the way to recover from a partial release.
failures=()

publish_npm || failures+=("npm")
publish_gem || failures+=("RubyGems")

if ((${#failures[@]} > 0)); then
  echo "failed to publish buildonce ${version} to: ${failures[*]}" >&2
  exit 1
fi
