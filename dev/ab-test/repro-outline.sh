#!/bin/bash
# Repro: 1.7B Qwen + nemotron + lecture audio — why no outline/questions?
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"
ev() { browser-eval --tab "$TAB" "$1" 2>&1 | tail -1; }

browser-start --background 2>&1 | tail -1
TAB=$(browser-nav http://localhost:8080 | grep "^tab:" | cut -d: -f2)
echo "TAB=$TAB"
sleep 4

# Mike's config: nemotron ASR + explicit 1.7B Qwen, bridge off, outline+questions on
ev "(() => { const s = JSON.parse(localStorage.getItem('silentNotetaker_settings') || '{}');
  Object.assign(s, { model: 'nemotron', qwenModel: '${QWEN_MODEL:-0.6b}', smartQuestions: true, liveOutline: true, claudeBridge: false, triggerDetection: true });
  localStorage.setItem('silentNotetaker_settings', JSON.stringify(s));
  localStorage.removeItem('qwenAutoDemoted');
  return 'settings staged (qwen=${QWEN_MODEL:-0.6b})'; })()"
browser-nav "http://localhost:8080" --tab "$TAB" >/dev/null   # reload with new settings
sleep 5

# Console capture BEFORE anything runs
ev "(() => { window.__logs = []; for (const k of ['log','warn','error']) { const o = console[k].bind(console);
  console[k] = (...a) => { try { window.__logs.push(k + ': ' + a.map(x => String(x && x.message || x)).join(' ')); if (window.__logs.length > 400) window.__logs.shift(); } catch(_){} o(...a); }; }
  return 'console hooked'; })()"

# Inject the fake-mic harness + load the lecture clip
browser-eval --tab "$TAB" "$(cat "$DIR/inject.js")" 2>&1 | tail -1
ev "__ab.setup('http://localhost:8080/dev/ab-test/${CLIP:-lecture_1x.wav}')"

ev "app.start(); 'starting'"
echo "[repro] waiting for nemotron ready..."
for i in $(seq 1 150); do
  R=$(ev "!!(app.tm && app.tm.nemotron && app.tm.nemotron.engine)")
  [ "$R" = "true" ] && break
  sleep 2
done
[ "$R" != "true" ] && { echo "[repro] FAIL: engine never ready"; ev "JSON.stringify((window.__logs||[]).slice(-15))"; exit 1; }
echo "[repro] nemotron ready; rolling audio"
ev "window.__ab.begin()"

# Poll: transcript progress + qwen worker + outline + smartq state
for i in $(seq 1 16); do
  sleep 20
  S=$(ev "JSON.stringify({ ab: JSON.parse(__ab.status()), qwen: { ready: sharedQuestionGenerator.ready, model: (sharedQuestionGenerator.config.model||'').split('/').pop(), device: sharedQuestionGenerator.config.device, pending: sharedQuestionGenerator._pending.size }, outline: { enabled: LiveOutline.enabled, buf: LiveOutline._buf.length, busy: LiveOutline._busy, fails: LiveOutline._fails, notes: LiveOutline._notes.length, status: (document.getElementById('liveOutlineStatus')||{}).textContent || '' }, smartq: { enabled: SmartQ.enabled, busy: !!SmartQ._busy, accum: SmartQ._accumChars || 0, shown: (document.getElementById('smartqText')||{}).textContent || '', empty: (document.getElementById('smartqEmpty')||{}).textContent || '' } })")
  echo "[repro] t=$((i*20))s $S"
  case "$S" in *'"ended":true'*) break;; esac
done

echo "[repro] === recent console ==="
ev "JSON.stringify((window.__logs||[]).slice(-25))"
echo "[repro] === outline DOM ==="
ev "JSON.stringify({ visible: (document.getElementById('liveOutline')||{style:{}}).style.display, groups: [...document.querySelectorAll('.live-outline-group')].map(g => g.querySelector('.live-outline-topic').textContent) })"
ev "app.stop(); 'stopped'"
echo "[repro] DONE (browser left running for inspection)"
