#!/usr/bin/env bash
set -euo pipefail

required_tools=(
  swift
  swiftc
  clang
  clang++
  ld.lld
  ld64.lld
  llvm-ar
  llvm-libtool-darwin
  llvm-lipo
  llvm-ranlib
  actool
  ibtool
  plutil
  rcodesign
  ruby
  zip
)

for tool in "${required_tools[@]}"; do
  command -v "${tool}" >/dev/null
done

swift --version
clang --version
ld64.lld --version
llvm-lipo --version
llvm-libtool-darwin --version
actool --version
ibtool --help >/dev/null
plutil --version
rcodesign --version
python3 -c 'import lxml'
ruby --version
zip -v >/dev/null

toolchain_dir=/usr/local/share/once-apple-linux-toolchain
manifest="${toolchain_dir}/toolchain.json"
plutil -convert json -o - "${toolchain_dir}/verification.plist" | grep -F '"Once": "verified"' >/dev/null
grep -F '"included": false' "${manifest}" >/dev/null

for prohibited_path in /Applications/Xcode.app /opt/Xcode.app /opt/AppleSDK; do
  if [[ -e "${prohibited_path}" ]]; then
    echo "Apple platform content is not permitted in this image: ${prohibited_path}" >&2
    exit 1
  fi
done

for prohibited_tool in codesign derq intentbuilderc ipatool momc xcodebuild xcrun; do
  if command -v "${prohibited_tool}" >/dev/null; then
    echo "Apple platform tooling is not permitted in this image: ${prohibited_tool}" >&2
    exit 1
  fi
done
