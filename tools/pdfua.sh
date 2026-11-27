#!/usr/bin/env bash
# Renders every fixture as a tagged PDF and checks it against PDF/UA-1 with veraPDF.
#
#   cargo build --release
#   tools/pdfua.sh                 # needs Docker (image verapdf/cli)
#
# Writes target/pdfua/<fixture>.pdf and target/pdfua/report.xml, prints each rule that fails
# and in which files; exit status 1 when a file fails.
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
bin=${FERROREPORT_BIN:-$root/target/release/ferroreport}
out=$root/target/pdfua
rm -rf "$out" && mkdir -p "$out"
for dir in "$root"/fixtures/*/; do
  name=$(basename "$dir")
  args=("$dir/report.jrxml" --tagged -q -o "$out/$name.pdf" --now 2024-01-15T10:30:00)
  [ -f "$dir/data.json" ] && args+=(-d "$dir/data.json")
  [ -f "$dir/params.json" ] && args+=(-p "$dir/params.json")
  # Fonts a fixture needs (fonts.txt: Family=file.ttf[:bold][:italic]), when installed.
  if [ -f "$dir/fonts.txt" ]; then
    while IFS= read -r f; do p=${f#*=}; [ -n "$f" ] && [ -f "${p%%:*}" ] && args+=(--font "$f"); done < "$dir/fonts.txt"
  fi
  # A JasperReports font extension the fixture ships (jasper-extension/), as a resource folder.
  [ -d "$dir/jasper-extension" ] && args+=(-r "$dir/jasper-extension")
  if [ -f "$dir/fontsets.txt" ]; then
    while IFS= read -r f; do [ -n "$f" ] && args+=(--font-set "$f"); done < "$dir/fontsets.txt"
  fi
  "$bin" render "${args[@]}" 2>/dev/null || echo "$name: not rendered"
done
docker run --rm -v "$out":/data verapdf/cli:latest --flavour ua1 --format xml --maxfailuresdisplayed 3 /data > "$out/report.xml" 2>/dev/null
python3 "$root/tools/pdfua_summary.py" "$out/report.xml"
