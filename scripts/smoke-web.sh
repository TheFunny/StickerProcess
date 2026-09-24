#!/usr/bin/env bash
# Mount the final Pages artifact in a real headless browser.
set -euo pipefail

artifact="${1:-web-dist}"
[[ -f "$artifact/index.html" ]] || {
  echo "missing $artifact/index.html" >&2
  exit 1
}

staging=$(mktemp -d)
server_pid=""
cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$staging"
}
trap cleanup EXIT

# Dioxus is built for /StickerProcess/; serve the artifact one directory below
# the server root so its absolute asset URLs resolve exactly as on Pages.
mkdir -p "$staging/StickerProcess"
cp -R "$artifact/." "$staging/StickerProcess/"

python_bin="${PYTHON_BIN:-python3}"
port=$("$python_bin" - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)
"$python_bin" -m http.server "$port" --bind 127.0.0.1 --directory "$staging" >"$staging/server.log" 2>&1 &
server_pid=$!

url="http://127.0.0.1:$port/StickerProcess/"
for _ in {1..50}; do
  curl -fsS "$url" >/dev/null && break
  sleep 0.1
done
curl -fsS "$url" >/dev/null

chrome="${CHROME_BIN:-}"
if [[ -z "$chrome" ]]; then
  for candidate in google-chrome google-chrome-stable chromium chromium-browser; do
    if command -v "$candidate" >/dev/null 2>&1; then
      chrome=$candidate
      break
    fi
  done
fi
[[ -n "$chrome" ]] || {
  echo "Chrome/Chromium is required for the web mount smoke test" >&2
  exit 1
}

printf 'browser: '
"$chrome" --version

"$chrome" \
  --headless=new \
  --no-sandbox \
  --disable-gpu \
  --disable-dev-shm-usage \
  --no-first-run \
  --no-default-browser-check \
  --user-data-dir="$staging/chrome-profile" \
  --virtual-time-budget=10000 \
  --dump-dom "$url" >"$staging/dom.html"

"$python_bin" - "$staging/dom.html" <<'PY'
import sys
from pathlib import Path
html = Path(sys.argv[1]).read_text(encoding="utf-8")
assert 'class="app"' in html, "Dioxus app did not mount"
assert "Drop files here" in html, "app empty state did not render"
PY
