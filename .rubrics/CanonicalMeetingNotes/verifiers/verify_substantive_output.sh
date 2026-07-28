#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node --input-type=module <<'NODE'
import { finalNotesToMarkdown, parseFinalNotes } from './final-notes.mjs';
const notes = parseFinalNotes(`OVERVIEW| The team selected Postgres, planned the migration, and retained one latency question.
SECTION| Database choice
SUMMARY| Relational data and future flexibility drove the database choice.
DECISION| Use Postgres for the user database.
ACTION| Sarah — prepare the migration script — Thursday
KEYPOINT| Read latency reached 800 milliseconds at p99.
QUESTION| Will connection pooling reduce p99 latency?`, 'Database choice');
const markdown = finalNotesToMarkdown(notes, {
  title: 'Architecture Review',
  date: 'July 27, 2026',
  duration: '32:10',
});
if (markdown.length < 300) throw new Error(`output too short: ${markdown.length}`);
for (const heading of ['## Discussion', '## Decisions', '## Action Items', '## Open Questions']) {
  if (!markdown.includes(heading)) throw new Error(`missing ${heading}`);
}
console.log(markdown.length);
NODE
