#!/bin/bash
# Drive the 200s nemotron repro: wait for engine ready, roll clip, stop, scrape.
TAB=$1
OUT=$2
ev() { browser-eval --tab "$TAB" "$1" 2>&1; }

echo "[run-4x] waiting for nemotron ready..."
for i in $(seq 1 120); do
  R=$(ev "!!(app.tm && app.tm.nemotron && app.tm.nemotron.engine)")
  [ "$R" = "true" ] && break
  sleep 2
done
[ "$R" != "true" ] && { echo "[run-4x] FAIL: engine never ready"; exit 1; }
echo "[run-4x] engine ready, beginning audio"
ev "window.__ab.begin()"

# Poll status every 10s until clip ends; log progress + live item count + stats.
for i in $(seq 1 30); do
  sleep 10
  S=$(ev "window.__ab.status()")
  echo "[run-4x] t=$((i*10))s $S"
  case "$S" in *'"ended":true'*) break;; esac
done

# Let the engine drain its feed backlog before stopping (decode may lag live audio).
echo "[run-4x] clip ended; draining backlog"
for i in $(seq 1 60); do
  P=$(ev "app.tm.nemotron.stats().pendingSamples")
  [ "$P" = "0" ] && break
  sleep 2
done
echo "[run-4x] pending=$P; stopping"
ev "app.stop(); 'stopping'"
sleep 8
ev "window.__ab.scrape()" > "$OUT"
echo "[run-4x] scraped $(wc -c < "$OUT") bytes -> $OUT"
# Grab perf stats + any console-visible drop evidence
ev "JSON.stringify(app.tm.nemotron.stats())"
echo "[run-4x] DONE"
