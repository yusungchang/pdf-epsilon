#!/bin/sh
# Screenshot the interface in headless Chrome, without Tauri.
# mock.js stands in for window.__TAURI__. The PDFs given are analysed with the real
# engine (the inspect example) and their reports are shown; a fake encrypted file is added.
# The page is served over a local web server, because the interface fetches its language file.
# Thumbnails come from the app's own renderer process and one image comparison from the engine.
# PREVIEW_UPDATE=0.3.0 pretends to be that version and opens the update check (it asks GitHub for real).
# Usage: [PREVIEW_UPDATE=x.y.z] [PREVIEW_OPEN=1] [PREVIEW_CLEAN=1] [PREVIEW_SCALE=2] [PREVIEW_LANG=ko] [PREVIEW_COMPARE=page|diff|diff4] [PREVIEW_SLOW=1] tools/ui-preview/preview.sh out.png [light|dark] [height] file.pdf...
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$PWD/ui-preview.png}
scheme=${2:-light}
height=${3:-1000}
shift 3 2>/dev/null || shift $#
work=$(mktemp -d)
cp -R "$root"/ui/. "$work"/
cp "$here/mock.js" "$work"/
(cd "$root" && cargo build -q -p epsilon --release --example inspect && cargo build -q -p pdf-epsilon --release)
inspect="$root/target/release/examples/inspect"
printf 'window.__ANALYSES__ = {\n' > "$work/analyses.js"
for pdf in "$@"; do
  printf '%s: ' "\"$(basename "$pdf")\"" >> "$work/analyses.js"
  "$inspect" "$pdf" --json >> "$work/analyses.js"
  printf ',\n' >> "$work/analyses.js"
done
printf '};\n' >> "$work/analyses.js"
[ -n "$PREVIEW_LANG" ] && printf 'window.__LANG__ = "%s";\n' "$PREVIEW_LANG" >> "$work/analyses.js"
printf 'window.__THEME__ = "%s";\n' "$scheme" >> "$work/analyses.js"
[ -n "$PREVIEW_COMPARE" ] && printf 'window.__COMPARE__ = "%s";\n' "$PREVIEW_COMPARE" >> "$work/analyses.js"
[ -n "$PREVIEW_SLOW" ] && printf 'window.__SLOW__ = true;\n' >> "$work/analyses.js"
[ -n "$PREVIEW_UPDATE" ] && printf 'window.__VERSION__ = "%s"; window.__UPDATE__ = true;\n' "$PREVIEW_UPDATE" >> "$work/analyses.js"
[ -n "$PREVIEW_OPEN" ] && printf 'try { localStorage.setItem("moreOpen", "true"); } catch {}\n' >> "$work/analyses.js"
[ -n "$PREVIEW_CLEAN" ] && printf 'window.__CLEAN__ = true;\n' >> "$work/analyses.js"
(cd "$root" && cargo build -q -p epsilon --release --example squeeze)
python3 -I "$here/prepare.py" "$root/target/release/pdf-epsilon" "$inspect" "$root/target/release/examples/squeeze" "$work/extras.js" "$@"
# No transitions: a screenshot must not catch a panel halfway through one.
sed -i.bak 's|</head>|<style>*, *::before, *::after { transition: none !important; }</style></head>|' "$work/index.html"
sed -i.bak 's|<script src="app.js"></script>|<script src="analyses.js"></script><script src="extras.js"></script><script src="mock.js"></script><script src="app.js"></script>|' "$work/index.html"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work" >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -rf "$work"' EXIT
sleep 0.5
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
# perl's alarm is a portable timeout: headless Chrome can hang on some flags.
perl -e 'alarm 60; exec @ARGV' "$chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor="${PREVIEW_SCALE:-1}" \
  --user-data-dir="$work/profile" --window-size=820,"$height" --virtual-time-budget=6000 \
  --blink-settings=preferredColorScheme=$([ "$scheme" = dark ] && echo 0 || echo 1) \
  --enable-logging=stderr --screenshot="$out" "http://127.0.0.1:$port/index.html" 2>&1 | grep -iE "uncaught|written" || true
