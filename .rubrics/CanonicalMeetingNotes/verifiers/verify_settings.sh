#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node <<'NODE'
const fs = require('fs');
const html = fs.readFileSync('index.html', 'utf8');
const settings = html.slice(html.indexOf('function openSettings()'), html.indexOf('function applySettings()'));
for (const retired of [
  'Auto Note Detection',
  'Auto Summary on Stop',
  'AI Final Notes',
  'Question Recap on Stop',
  'settingTriggers',
  'settingSummary',
  'settingAiNotes',
  'settingSmartQRecap',
]) {
  if (settings.includes(retired)) throw new Error(`retired setting remains: ${retired}`);
}
for (const expected of ['Live Draft', 'Advanced AI prompts', 'Advanced bridge connection']) {
  if (!settings.includes(expected)) throw new Error(`missing progressive setting: ${expected}`);
}
const defaults = html.slice(html.indexOf('const DEFAULT_SETTINGS'), html.indexOf('function loadSettings()'));
for (const retired of ['triggerDetection:', 'autoSummary:', 'aiFinalNotes:', 'smartqRecap:']) {
  if (defaults.includes(retired)) throw new Error(`retired default remains: ${retired}`);
}
console.log('settings distilled');
NODE
