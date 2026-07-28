import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

import {
  finalNotesToMarkdown,
  parseFinalNotes,
} from '../../../final-notes.mjs';

const fixture = JSON.parse(
  fs.readFileSync(new URL('./held-out.json', import.meta.url), 'utf8'),
);

test('held-out agenda reconciliation preserves omissions and grounding', () => {
  const notes = parseFinalNotes(fixture.raw, fixture.agenda);
  assert.deepEqual(
    notes.sections.map((section) => section.title),
    ['Vendor readiness', 'Rollout timing', 'Accessibility review', 'Other discussion'],
  );
  assert.equal(notes.sections[2].status, 'not discussed');
  const markdown = finalNotesToMarkdown(notes, { title: 'Held-out Review' });
  for (const forbidden of fixture.forbidden) assert.doesNotMatch(markdown, new RegExp(forbidden, 'i'));
  assert.match(markdown, /Support — publish the escalation guide/);
});
