#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node <<'NODE'
const fs = require('fs');
const report = JSON.parse(fs.readFileSync('.evals/MeetingNotesQuality/runs/browser-smoke.json', 'utf8'));
for (const key of [
  'agendaEditorVisible',
  'agendaCountCorrect',
  'retiredSettingsAbsent',
  'advancedSettingsCollapsed',
  'finalNotesRendered',
  'finalNotesTitleCorrect',
  'notDiscussedVisible',
  'consoleErrorsEmpty',
]) {
  if (report[key] !== true) throw new Error(`browser smoke failed: ${key}`);
}
if (!String(report.screenshot || '').endsWith('.png')) throw new Error('visual screenshot evidence missing');
console.log('browser smoke passed');
NODE
