#!/usr/bin/env bash
# Static web assets for the offline map (DESIGN §16), pinned and checksum-verified:
#   maplibre-gl (renderer), @protomaps/basemaps (style layers), protomaps/basemaps-assets (glyphs, sprites).
# Usage: scripts/fetch_map_assets.sh [OUT_DIR]   (default ~/sp-scratch/maps/assets)
set -euo pipefail
OUT=${1:-$HOME/sp-scratch/maps/assets}
MAPLIBRE=6.16.0
MAPLIBRE_SHA512=gApO66EMtoDZuio680RUBvskR7ahFd/NNhLN4tBAs+Y7FeT4gQV8zVFgYVMRmFxQznOEzecW19w6xOd9XjZjiA==
BASEMAPS=5.7.2
BASEMAPS_SHA512=K1Yk6bWdULulYg+R2QRVXx4NzJZan5YQhpejEG0c1/sXruJrfPIPZuakpf3jwAgVmjIRVQwAv+yRafDeN0aaUQ==
ASSETS_COMMIT=028c18f713baecad011301ff7a69acc39bcc2ae7
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
mkdir -p "$OUT"

npm_pkg() { # name url sha512 subdir
  curl -sSLf -o "$TMP/$1.tgz" "$2"
  echo "$3  $TMP/$1.tgz" | python3 -c "
import base64,hashlib,sys
want,path=sys.stdin.read().split()
got=base64.b64encode(hashlib.sha512(open(path,'rb').read()).digest()).decode()
sys.exit(0 if got==want else f'sha512 mismatch for {path}')"
  mkdir -p "$TMP/$1" "$OUT/$4"
  tar xzf "$TMP/$1.tgz" -C "$TMP/$1"
}
npm_pkg maplibre https://registry.npmjs.org/maplibre-gl/-/maplibre-gl-$MAPLIBRE.tgz $MAPLIBRE_SHA512 maplibre-gl
# v6 ships ES modules: the entry imports the shared chunk and starts the worker module
cp "$TMP/maplibre/package/dist/"maplibre-gl{,-shared,-worker}.mjs "$TMP/maplibre/package/dist/maplibre-gl.css" \
   "$TMP/maplibre/package/LICENSE.txt" "$OUT/maplibre-gl/"
npm_pkg basemaps https://registry.npmjs.org/@protomaps/basemaps/-/basemaps-$BASEMAPS.tgz $BASEMAPS_SHA512 basemaps
ls "$TMP/basemaps/package/dist"
cp "$TMP/basemaps/package/dist/basemaps.js" "$OUT/basemaps/" 2>/dev/null || cp "$TMP/basemaps/package/dist/"*.js "$OUT/basemaps/"
cp "$TMP/basemaps/package/LICENSE"* "$OUT/basemaps/" 2>/dev/null || true

curl -sSLf -o "$TMP/assets.tgz" "https://codeload.github.com/protomaps/basemaps-assets/tar.gz/$ASSETS_COMMIT"
tar xzf "$TMP/assets.tgz" -C "$TMP"
rm -rf "$OUT/fonts" "$OUT/sprites"
cp -r "$TMP/basemaps-assets-$ASSETS_COMMIT/fonts" "$TMP/basemaps-assets-$ASSETS_COMMIT/sprites" "$OUT/"
cp "$TMP/basemaps-assets-$ASSETS_COMMIT/"{LICENSE*,README*} "$OUT/" 2>/dev/null || true
printf '{"maplibre-gl":"%s","@protomaps/basemaps":"%s","basemaps-assets":"%s"}\n' $MAPLIBRE $BASEMAPS $ASSETS_COMMIT > "$OUT/versions.json"
du -sh "$OUT"/*
