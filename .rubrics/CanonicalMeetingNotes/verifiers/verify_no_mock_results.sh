#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
if [[ ! -f .evals/MeetingNotesQuality/runs/latest.json ]]; then
  exit 0
fi
node <<'NODE'
const fs = require('fs');
const run = JSON.parse(fs.readFileSync('.evals/MeetingNotesQuality/runs/latest.json', 'utf8'));
if (run.mock === true || run.fallback === true) throw new Error('mock or fallback result cannot count');
if (run.status === 'complete') {
  if (run.backend !== 'transformers.js') throw new Error('complete run lacks real Transformers.js backend');
  if (!Array.isArray(run.items) || run.items.some(item => !String(item.rawOutput || '').trim())) {
    throw new Error('complete run lacks raw model evidence');
  }
}
console.log('no mock result admitted');
NODE
