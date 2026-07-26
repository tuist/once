#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"

cd "$root/bazel"
mise exec -- bazelisk build //:distribution \
  --remote_cache=grpc://127.0.0.1:19092 \
  --remote_instance_name=benchmark-bazel \
  --remote_upload_local_results=true \
  --remote_download_outputs=toplevel \
  --action_env=PATH \
  --noshow_progress \
  --color=no
