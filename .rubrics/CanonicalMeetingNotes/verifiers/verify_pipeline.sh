#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node <<'NODE'
const fs = require('fs');
const html = fs.readFileSync('index.html', 'utf8');
const start = html.indexOf('  async generateFinalNotes()');
const end = html.indexOf('  _renderFinalNotesModal(', start);
if (start < 0 || end < 0) throw new Error('generateFinalNotes method not found');
const body = html.slice(start, end);

const required = [
  'policy.extractGroundedEvidence(transcriptSegments)',
  'policy.formatMappedEvidence(dedupeNotes(mapped))',
  'policy.buildFinalNotesRequest({',
  'system: policy.FINAL_NOTES_SYSTEM',
  'policy.parseNaturalFinalNotes(',
  'policy.mergeEvidenceIntoFinalNotes(',
  'policy.finalNotesHasSubstance(notes)',
  'notes.modelOverviewUsed',
  'await storage.saveFinalNotes(this.meetingId, markdown)',
];
for (const needle of required) {
  if (!body.includes(needle)) throw new Error(`missing final pipeline boundary: ${needle}`);
}
const synthesisCalls = (body.match(/recapQuestionGenerator\.generate\(/g) || []).length;
if (synthesisCalls !== 1) {
  throw new Error(`expected one whole-meeting model call, got ${synthesisCalls}`);
}
if (/prefix\s*:/.test(body)) {
  throw new Error('structured response seeding is forbidden in natural final synthesis');
}
if (!body.includes('transcript.length > policy.DIRECT_TRANSCRIPT_MAX_CHARS')) {
  throw new Error('long-meeting evidence boundary missing');
}
console.log('one natural whole-meeting synthesis pass');
NODE
node --test tests/final-notes-policy.test.mjs >/dev/null
