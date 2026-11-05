#!/usr/bin/env bash
# Start (or resume) all source downloads, detached from any terminal.
# Safe to run repeatedly: a lock prevents duplicates, finished files are skipped, partial files resume.
# Logs: /mnt/bigbelly/survivalpedia/sources/fetch_*.log
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=/mnt/bigbelly/survivalpedia/sources

# Never write to the empty mount directory on the root disk if BigBelly isn't attached.
for i in $(seq 1 30); do mountpoint -q /mnt/bigbelly && break; sleep 10; done
mountpoint -q /mnt/bigbelly || { echo "$(date) BigBelly not mounted; not starting" >> "$HOME/.survivalpedia-fetch.err"; exit 1; }

start() {  # start <name> <command...>   one instance per name, guarded by flock
  local name=$1; shift
  setsid nohup flock -n "/tmp/sp-fetch-$name.lock" "$@" >> "$SRC/fetch_$name.log" 2>&1 < /dev/null &
}
start main       "$HERE/fetch_sources.sh"
start extra      "$HERE/fetch_sources.sh" "$HERE/sources_extra.txt"
start wiktionary "$HERE/fetch_sources.sh" "$HERE/sources_wiktionary.txt"
start proto      "$HERE/fetch_sources.sh" "$HERE/sources_proto.txt"
start meta       "$HERE/fetch_wikimedia_meta.sh"
echo "started; logs in $SRC/fetch_*.log"
