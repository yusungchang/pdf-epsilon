#!/bin/sh
# Screenshot the interface in headless Chrome, without Tauri.
# mock.js stands in for window.__TAURI__. The PDFs given are analysed with the real
# engine (the inspect example) and their reports are shown; a fake encrypted file is added.
# Usage: tools/ui-preview/preview.sh out.png [light|dark] [height] file.pdf...
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$PWD/ui-preview.png}
scheme=${2:-light}
height=${3:-1000}
shift 3 2>/dev/null || shift $#
work=$(mktemp -d)
cp "$root"/ui/* "$work"/
cp "$here/mock.js" "$work"/
(cd "$root" && cargo build -q -p epsilon --release --example inspect)
inspect="$root/target.nosync/release/examples/inspect"
printf 'window.__ANALYSES__ = {\n' > "$work/analyses.js"
for pdf in "$@"; do
  printf '%s: ' "\"$(basename "$pdf")\"" >> "$work/analyses.js"
  "$inspect" "$pdf" --json >> "$work/analyses.js"
  printf ',\n' >> "$work/analyses.js"
done
printf '};\n' >> "$work/analyses.js"
sed -i.bak 's|<script src="app.js"></script>|<script src="analyses.js"></script><script src="mock.js"></script><script src="app.js"></script>|' "$work/index.html"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
# perl's alarm is a portable timeout: headless Chrome can hang on some flags.
perl -e 'alarm 60; exec @ARGV' "$chrome" --headless=new --disable-gpu --hide-scrollbars \
  --user-data-dir="$work/profile" --window-size=820,"$height" --virtual-time-budget=6000 \
  --force-prefers-color-scheme="$scheme" --allow-file-access-from-files \
  --enable-logging=stderr --screenshot="$out" "file://$work/index.html" 2>&1 | grep -iE "uncaught|written" || true
rm -rf "$work"
