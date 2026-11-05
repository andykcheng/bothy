#!/usr/bin/env bash
# Builds the third-party games (DESIGN §17) into OUT/<id>/, ready to sit next to app/assets/games/*
# in the hub's --games directory. They are kept out of git: large, and rebuilt from upstream.
#   scripts/third_party_games.sh OUT [WORK]
#  - puzzles:  Simon Tatham's Portable Puzzle Collection, prebuilt web version (MIT) + source tarball
#  - farmhand: Farmhand by Jeremy Kahn (GPL-2.0-or-later), built with vite + source tarball
#  - dopewars: DopeWars.js by Nicolas Hurtubise (MIT), static files, one small patch
# Needs curl, git, node/npm, python3.
set -euo pipefail
OUT=$(realpath -m "$1"); WORK=$(realpath -m "${2:-$OUT/.work}")
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT" "$WORK"

# ---- Simon Tatham's puzzles ------------------------------------------------------------------
P=https://www.chiark.greenend.org.uk/~sgtatham/puzzles
T="$WORK/tatham"; mkdir -p "$T/js" "$T/doc"
[ -s "$T/jsindex.html" ] || curl -sfL --retry 3 "$P/js/" -o "$T/jsindex.html"
for f in $(grep -o 'href="[a-z0-9]*\.\(html\|js\|wasm\)"' "$T/jsindex.html" | cut -d'"' -f2); do
  [ -s "$T/js/$f" ] || curl -sfL --retry 3 "$P/js/$f" -o "$T/js/$f"
done
[ -s "$T/src.tar.gz" ] || curl -sfL --retry 3 "$P/puzzles.tar.gz" -o "$T/src.tar.gz"
G="$OUT/puzzles"; rm -rf "$G"; mkdir -p "$G/doc"
# only the finished puzzles: those with a help page ("group" and other unfinished ones have none)
for n in $(ls "$T/js"/*.html | xargs basename -s .html); do
  if [ ! -s "$T/doc/$n.html" ] && [ ! -e "$T/doc/$n.none" ]; then   # (cached; .none = upstream has no page: HTTP error 22)
    rc=0; curl -sfL --retry 3 "$P/doc/$n.html" -o "$T/doc/$n.html" || rc=$?
    if [ $rc = 22 ]; then rm -f "$T/doc/$n.html"; touch "$T/doc/$n.none"; elif [ $rc != 0 ]; then echo "download failed: $n ($rc)" >&2; exit 1; fi
  fi
  [ -s "$T/doc/$n.html" ] || continue
  cp "$T/js/$n".{html,js,wasm} "$G/"
  sed -i -e 's#href="\.\./doc/#href="doc/#g' -e 's#href="\.\./"#href="./"#g' "$G/$n.html"
  sed -e 's#href="index.html"#href="../"#g; s#href="indexpage.html"#href="../"#g' "$T/doc/$n.html" > "$G/doc/$n.html"
  # the pages set no background: the app's WebView is dark underneath, which left black text on black
  sed -i 's|</head>|<style>html { background: #fff; color: #111; color-scheme: light; }</style></head>|' "$G/$n.html"
  sed -i 's|</head>|<meta name="viewport" content="width=device-width, initial-scale=1"><style>html { background: #fff; color: #111; color-scheme: light; } body { line-height: 1.45; margin: 12px; }</style></head>|' "$G/doc/$n.html"
done
tar xzf "$T/src.tar.gz" -C "$T" --wildcards '*/LICENCE'
cp "$T"/puzzles-*/LICENCE "$G/LICENCE"; cp "$T/src.tar.gz" "$G/puzzles-source.tar.gz"
VER=$(ls -d "$T"/puzzles-* | head -1 | sed 's#.*/puzzles-##')
python3 "$HERE/puzzles_index.py" "$G" "$VER"

# ---- Farmhand ----------------------------------------------------------------------------------
F="$WORK/farmhand"
[ -d "$F" ] || git clone -q --depth 1 https://github.com/jeremyckahn/farmhand.git "$F"
( cd "$F" && ELECTRON_SKIP_BINARY_DOWNLOAD=1 PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund >/dev/null && npx vite build --base=./ >/dev/null )
FV=$(node -p "require('$F/package.json').version"); FC=$(git -C "$F" rev-parse --short HEAD)
G="$OUT/farmhand"; rm -rf "$G"; mkdir -p "$G"
rsync -a --exclude '*.map' --exclude electron.js --exclude service-worker.js --exclude 'workbox-*' "$F/dist/" "$G/"
git -C "$F" archive --format=tar.gz --prefix="farmhand-$FV/" -o "$G/farmhand-source.tar.gz" HEAD
cp "$F/LICENSE" "$G/LICENSE"
cat > "$G/game.json" <<JSON
{
  "id": "farmhand",
  "title": "Farmhand",
  "description": "Plant, water and harvest crops, raise cows, sell at a changing market and grow your farm. Saves automatically.",
  "players": "1",
  "version": 1,
  "license": "GPL-2.0-or-later",
  "author": "Jeremy Kahn and contributors (github.com/jeremyckahn/farmhand)",
  "source": "farmhand-source.tar.gz"
}
JSON
cat > "$G/README.txt" <<TXT
Farmhand $FV (commit $FC) by Jeremy Kahn and contributors.
https://github.com/jeremyckahn/farmhand. Licence: GPL-2.0-or-later (LICENSE).

The included build is unmodified apart from leaving out source maps, the Electron entry point and the
service worker. Its complete source is in farmhand-source.tar.gz. Rebuild it with:
  npm ci && npx vite build --base=./
Online features (multiplayer, shared market) need the internet and are unavailable offline.
TXT
# ---- DopeWars.js ----------------------------------------------------------------------------------
D="$WORK/dopewars"
[ -d "$D" ] || git clone -q --depth 1 https://github.com/316k/DopeWars.js.git "$D"
DC=$(git -C "$D" rev-parse --short HEAD)
G="$OUT/dopewars"; rm -rf "$G"; mkdir -p "$G"
rsync -a --exclude .git --exclude '*.xcf' --exclude 'screen*.png' --exclude '*.map' --exclude README.md --exclude TODO.md --exclude manifest.webapp "$D/" "$G/"
# it re-asks for a name until it gets one; the app's WebView answers a cancelled prompt with '' (endless loop)
python3 - "$G/assets/js/controller.js" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
a = '''    var name = prompt(navigator.mozL10n.get('prompt-name'));

    while(!name) {
        name = prompt(navigator.mozL10n.get('prompt-name-again'));
    }
'''
assert a in s, 'DopeWars.js changed upstream: re-check the name prompt patch'
s = s.replace(a, '''    // Survivalpedia: no endless re-asking when the prompt is cancelled (the app's WebView answers '').
    var name = prompt(navigator.mozL10n.get('prompt-name')) || 'Player';
''')
open(p, 'w').write(s)
PY
cat > "$G/game.json" <<JSON
{
  "id": "dopewars",
  "title": "DopeWars",
  "description": "The classic trading game: 30 days to buy low and sell high around the city, dodge the cops and pay off the loan shark. Adult theme (drugs).",
  "players": "1",
  "version": 1,
  "license": "MIT",
  "author": "Nicolas Hurtubise (github.com/316k/DopeWars.js), after John E. Dell's Drug Wars"
}
JSON
cat > "$G/README.txt" <<TXT
DopeWars.js (commit $DC) by Nicolas Hurtubise. https://github.com/316k/DopeWars.js. Licence: MIT (LICENSE).
Bundled libraries: jQuery, Bootstrap (incl. Glyphicons Halflings font) and readmore.js (MIT), l10n.js (webL10n).
Changed for Survivalpedia: assets/js/controller.js no longer re-asks for a name forever when the prompt
is cancelled (it uses "Player"). Development files (screenshots, .xcf, source maps) are left out.
TXT

du -sh "$OUT"/puzzles "$OUT"/farmhand "$OUT"/dopewars

# Cover images for the Games grid (these game.json files are rewritten above, so covers are made again):
#   (cd "$OUT" && python3 -m http.server 47900 --bind 127.0.0.1 &)
#   node scripts/gameshot.js <playwright-core dir> http://127.0.0.1:47900/ "$OUT" puzzles farmhand dopewars
