import assert from 'node:assert/strict';
import test from 'node:test';

import {
  finalNotesToMarkdown,
  parseFinalNotes,
} from '../../final-notes.mjs';
import { scoreMeetingNotes } from './score-output.mjs';

const fixture = {
  expectedConcepts: [
    { id: 'db', all: [['postgres']] },
    { id: 'action', all: [['sarah'], ['migration'], ['thursday']] },
  ],
  forbiddenClaims: ['Alice'],
  expectedAgenda: [
    { title: 'Database', status: 'discussed' },
    { title: 'Security', status: 'not discussed' },
  ],
  expectedMinimums: { sections: 2, decisions: 1, actions: 1, questions: 0 },
};

test('grounded complete notes score above the pass threshold', () => {
  const notes = parseFinalNotes(`OVERVIEW| The team chose Postgres and assigned the migration.
SECTION| Database
SUMMARY| Relational data drove the database choice.
DECISION| Use Postgres.
ACTION| Sarah — prepare the migration — Thursday`, 'Database\nSecurity');
  const markdown = finalNotesToMarkdown(notes, { title: 'Review', date: 'Jul 27', duration: '20:00' });
  const result = scoreMeetingNotes(fixture, notes, markdown);
  assert.equal(result.pass, true);
  assert.ok(result.score >= 0.9, JSON.stringify(result));
});

test('invented owner fails grounding even with good coverage', () => {
  const notes = parseFinalNotes(`OVERVIEW| The team chose Postgres and assigned the migration.
SECTION| Database
SUMMARY| Relational data drove the database choice.
DECISION| Use Postgres.
ACTION| Sarah — prepare the migration — Thursday
ACTION| Alice — approve security — Friday`, 'Database\nSecurity');
  const markdown = finalNotesToMarkdown(notes, { title: 'Review' });
  const result = scoreMeetingNotes(fixture, notes, markdown);
  assert.equal(result.dimensions.grounding, 0);
  assert.equal(result.pass, false);
});

test('missing agenda coverage lowers agenda and structure scores', () => {
  const notes = {
    overview: 'The team discussed a possible database.',
    sections: [{
      title: 'Discussion',
      status: 'discussed',
      summary: 'Postgres came up.',
      decisions: [],
      actions: [],
      keypoints: [],
      questions: [],
    }],
  };
  const result = scoreMeetingNotes(fixture, notes, '# Review\n\nPostgres came up.');
  assert.ok(result.dimensions.agenda < 1);
  assert.equal(result.pass, false);
});

test('an explicitly unresolved cause does not trigger a causal invention', () => {
  const causalFixture = {
    expectedConcepts: [],
    forbiddenClaims: ['payment gateway caused the slowdown'],
    expectedMinimums: { sections: 1, questions: 1 },
  };
  const notes = parseFinalNotes(`OVERVIEW| The incident cause remains unresolved.
SECTION| Incident
QUESTION| We do not yet know whether the payment gateway caused the slowdown.`);
  const markdown = finalNotesToMarkdown(notes, { title: 'Incident' });
  const result = scoreMeetingNotes(causalFixture, notes, markdown);
  assert.equal(result.dimensions.grounding, 1);
});

test('an asserted unsupported cause still fails grounding', () => {
  const causalFixture = {
    expectedConcepts: [],
    forbiddenClaims: ['payment gateway caused the slowdown'],
    expectedMinimums: { sections: 1 },
  };
  const notes = parseFinalNotes(`OVERVIEW| The incident cause was identified.
SECTION| Incident
SUMMARY| The payment gateway caused the slowdown.`);
  const markdown = finalNotesToMarkdown(notes, { title: 'Incident' });
  const result = scoreMeetingNotes(causalFixture, notes, markdown);
  assert.equal(result.dimensions.grounding, 0);
  assert.equal(result.pass, false);
});

test('an invented calendar value fails the generic grounding check', () => {
  const dateFixture = {
    transcriptSegments: ['The team agreed to launch on the 15th.'],
    expectedConcepts: [],
    forbiddenClaims: [],
    expectedMinimums: { sections: 1 },
  };
  const notes = parseFinalNotes(`OVERVIEW| The team agreed to launch on April 15th.
SECTION| Launch
DECISION| Launch on April 15th.`);
  const markdown = finalNotesToMarkdown(notes, { title: 'Launch' });
  const result = scoreMeetingNotes(dateFixture, notes, markdown);
  assert.equal(result.dimensions.grounding, 0);
  assert.deepEqual(result.unsupportedSpecifics, ['april']);
  assert.equal(result.pass, false);
});

test('a spelled-out amount may be rendered with zero-grouped digits', () => {
  const amountFixture = {
    transcriptSegments: ['The approved cap remains seven hundred fifty thousand dollars.'],
    expectedConcepts: [{ id: 'cap', all: [['750', 'seven hundred fifty'], ['cap']] }],
    forbiddenClaims: [],
    expectedMinimums: { sections: 1 },
  };
  const notes = parseFinalNotes(`OVERVIEW| The approved cap remains $750,000.
SECTION| Budget
DECISION| Keep the approved cap.`);
  const markdown = finalNotesToMarkdown(notes, { title: 'Budget' });
  const result = scoreMeetingNotes(amountFixture, notes, markdown);
  assert.equal(result.dimensions.grounding, 1);
  assert.deepEqual(result.unsupportedSpecifics, []);
});

test('agenda items must all match their expected status to pass', () => {
  const agendaFixture = {
    transcriptSegments: ['Budget was discussed.'],
    expectedConcepts: [],
    forbiddenClaims: [],
    expectedAgenda: [
      { title: 'Budget', status: 'discussed' },
      { title: 'Security', status: 'not discussed' },
    ],
    expectedMinimums: { sections: 2 },
  };
  const notes = parseFinalNotes(`OVERVIEW| The team discussed the budget.
SECTION| Budget
SUMMARY| Budget was discussed.
SECTION| Security
SUMMARY| Security was discussed.`, 'Budget\nSecurity');
  const markdown = finalNotesToMarkdown(notes, { title: 'Planning' });
  const result = scoreMeetingNotes(agendaFixture, notes, markdown);
  assert.ok(result.score >= 0.7);
  assert.equal(result.dimensions.agenda, 0.5);
  assert.equal(result.pass, false);
});
