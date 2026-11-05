#!/usr/bin/env bash
# Download source ZIMs (latest per prefix) to $DEST with resume + sha256 verification.
set -uo pipefail
DEST=${DEST:-/mnt/bigbelly/survivalpedia/sources}
LIVE=https://download.kiwix.org/zim
ARCH=https://archive.download.kiwix.org/zim
LIST=${1:+$(realpath "$1")}
cd "$DEST"

# base_url  dir  prefix (latest file matching prefix_YYYY-MM.zim is fetched)
SOURCES="
$ARCH wikihow wikihow_en_maxi
$LIVE ifixit ifixit_en_all
$LIVE other appropedia_en_all_maxi
$LIVE other energypedia_en_all_maxi
$LIVE other theworldfactbook_en_all
$LIVE wikivoyage wikivoyage_en_all_maxi
$LIVE wikipedia wikipedia_en_all_maxi
"

[ -n "$LIST" ] && SOURCES=$(sed "s#\$LIVE#$LIVE#; s#\$ARCH#$ARCH#" "$LIST")
echo "$SOURCES" | while read -r base dir prefix; do
  [ -z "$base" ] && continue
  f=$(curl -sL "$base/$dir/" | grep -oE "${prefix}_20[0-9]{2}-[0-9]{2}\.zim" | sort -u | tail -1)
  if [ -z "$f" ]; then echo "NOTFOUND $prefix"; continue; fi
  if [ -f "$f.ok" ]; then echo "SKIP $f"; continue; fi
  echo "GET $f  $(date +%T)"
  for try in 1 2 3 4 5; do
    aria2c -q -c -x 8 -s 8 --file-allocation=none --max-tries=10 --retry-wait=15 -o "$f" "$base/$dir/$f" && break
    echo "  retry $try"; sleep 30
  done
  want=$(curl -sLf "$base/$dir/$f.sha256" | awk '{print $1}')
  if [ -n "$want" ]; then
    got=$(sha256sum "$f" | awk '{print $1}')
    [ "$want" = "$got" ] && { touch "$f.ok"; echo "OK $f"; } || echo "BADSUM $f"
  else
    touch "$f.ok"; echo "OK(nosum) $f"
  fi
done
echo "DONE $(date +%T)"; df -h "$DEST" | tail -1
