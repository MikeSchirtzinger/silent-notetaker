#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node <<'NODE'
const fs = require('fs');
const path = '.evals/MeetingNotesQuality/runs/latest.json';
const run = JSON.parse(fs.readFileSync(path, 'utf8'));
if (run.status !== 'complete') throw new Error(`benchmark status: ${run.status}`);
if (run.mock === true || run.fallback === true) throw new Error('mock/fallback benchmark result');
if (run.backend !== 'transformers.js') throw new Error(`unexpected backend: ${run.backend}`);
if (!/^onnx-community\/Qwen3-/.test(run.model || '')) throw new Error(`unexpected model: ${run.model}`);
if (!Array.isArray(run.items) || run.items.length < 3) throw new Error('fewer than three real-model items');
if (run.items.some(item => !String(item.rawOutput || '').trim())) throw new Error('raw model output missing');
if (!Number.isFinite(run.passRate) || run.passRate < 0.75) throw new Error(`pass rate below 0.75: ${run.passRate}`);
if (!Number.isFinite(run.meanScore) || run.meanScore < 0.75) throw new Error(`mean score below 0.75: ${run.meanScore}`);
console.log(run.meanScore);
NODE
