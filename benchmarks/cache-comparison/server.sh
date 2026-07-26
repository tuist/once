#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
state="$root/.state"
remote="$state/remote"
cache_pid="$state/cache-server.pid"
control_pid="$state/control-server.pid"

start() {
  if curl -fsS http://127.0.0.1:18080/status >/dev/null 2>&1; then
    echo "cache server is already listening on port 18080" >&2
    exit 1
  fi

  mkdir -p "$remote"
  mise exec -- bazel-remote \
    --dir "$remote" \
    --max_size 2 \
    --http_address 127.0.0.1:18080 \
    --grpc_address 127.0.0.1:19092 \
    --enable_endpoint_metrics \
    --access_log_level all \
    --log_timezone none \
    >"$state/cache-server.log" 2>&1 &
  echo "$!" >"$cache_pid"

  (
    cd "$root/once"
    mise exec -- node control-server.mjs
  ) >"$state/control-server.log" 2>&1 &
  echo "$!" >"$control_pid"

  attempts=0
  until curl -fsS http://127.0.0.1:18080/status >/dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 100 ]; then
      echo "cache server did not become ready" >&2
      exit 1
    fi
    sleep 0.1
  done
}

stop() {
  for pid_file in "$control_pid" "$cache_pid"; do
    if [ -f "$pid_file" ]; then
      pid="$(sed -n '1p' "$pid_file")"
      if kill -0 "$pid" >/dev/null 2>&1; then
        kill "$pid"
        wait "$pid" 2>/dev/null || true
      fi
      find "$pid_file" -maxdepth 0 -type f -delete
    fi
  done
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  *)
    echo "usage: $0 start|stop" >&2
    exit 2
    ;;
esac
