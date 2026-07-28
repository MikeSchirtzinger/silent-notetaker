import assert from 'node:assert/strict';
import test from 'node:test';

import {
  DIRECT_TRANSCRIPT_MAX_CHARS,
  buildFinalNotesRequest,
  extractGroundedEvidence,
  finalNotesCounts,
  finalNotesHasSubstance,
  finalNotesToMarkdown,
  formatMappedEvidence,
  mergeEvidenceIntoFinalNotes,
  parseAgenda,
  parseEvidenceOutput,
  parseFinalNotes,
  parseNaturalFinalNotes,
} from '../final-notes.mjs';

test('agenda parser preserves order and removes list markers', () => {
  assert.deepEqual(
    parseAgenda('1. Database choice\n- Launch timing\n* Database choice\n[ ] Risks'),
    ['Database choice', 'Launch timing', 'Risks'],
  );
});

test('short transcript uses one direct whole-meeting request', () => {
  const request = buildFinalNotesRequest({
    transcriptSegments: ['We agreed to use Postgres.', 'Sarah will migrate it Thursday.'],
    agendaText: '',
  });
  assert.equal(request.mode, 'direct');
  assert.match(request.userText, /FULL TRANSCRIPT:/);
  assert.match(request.userText, /\[T1\] We agreed to use Postgres\./);
  assert.doesNotMatch(request.userText, /CHRONOLOGICAL EVIDENCE/);
});

test('short transcript can include an evidence index without replacing source', () => {
  const request = buildFinalNotesRequest({
    transcriptSegments: ['We agreed to use Postgres.', 'Sarah will migrate it Thursday.'],
    agendaText: '',
    evidence: '[E1] Database | DECISIONS | Use Postgres.',
  });
  assert.equal(request.mode, 'direct');
  assert.match(request.userText, /FULL TRANSCRIPT:/);
  assert.match(request.userText, /EXTRACTED EVIDENCE INDEX:/);
  assert.match(request.userText, /Use Postgres/);
});

test('long transcript requires chronological mapped evidence', () => {
  const transcript = `Long discussion. ${'x'.repeat(DIRECT_TRANSCRIPT_MAX_CHARS)}`;
  assert.throws(
    () => buildFinalNotesRequest({ transcript, agendaText: '', evidence: '' }),
    /no extracted evidence/,
  );

  const evidence = formatMappedEvidence([
    { part: 1, topic: 'Database', cat: 'decisions', text: 'Use Postgres.' },
    { part: 2, topic: 'Launch', cat: 'actions', text: 'Sarah — migrate data — Thursday.' },
  ]);
  const request = buildFinalNotesRequest({ transcript, agendaText: '', evidence });
  assert.equal(request.mode, 'reduce');
  assert.match(request.userText, /CHRONOLOGICAL EVIDENCE/);
  assert.match(request.userText, /\[E2\] Launch \| ACTIONS/);
});

test('agenda sections remain ordered and missing items are explicit', () => {
  const raw = `OVERVIEW| The team selected a database and assigned the migration.
SECTION| Launch timing
STATUS| discussed
SUMMARY| Launch timing moved to the fifteenth.
DECISION| Move launch to the fifteenth.
SECTION| Database choice
STATUS| discussed
SUMMARY| Relational data favored Postgres.
DECISION| Use Postgres.
ACTION| Sarah — prepare the migration script — Thursday`;

  const notes = parseFinalNotes(
    raw,
    'Database choice\nSecurity review\nLaunch timing',
  );
  assert.deepEqual(
    notes.sections.map((section) => section.title),
    ['Database choice', 'Security review', 'Launch timing'],
  );
  assert.equal(notes.sections[1].status, 'not discussed');
  assert.equal(notes.sections[0].decisions[0], 'Use Postgres.');
  assert.equal(notes.sections[2].decisions[0], 'Move launch to the fifteenth.');
});

test('off-agenda model sections collapse into one other-discussion section', () => {
  const notes = parseFinalNotes(
    `OVERVIEW| The planned review also surfaced a vendor blocker.
SECTION| Planned review
SUMMARY| The review passed.
SECTION| OAuth vendor
SUMMARY| Sandbox credentials are still missing.
QUESTION| When will the vendor provide credentials?`,
    'Planned review',
  );
  assert.deepEqual(
    notes.sections.map((section) => section.title),
    ['Planned review', 'Other discussion'],
  );
  assert.match(notes.sections[1].summary, /OAuth vendor/);
});

test('markdown reads as normal notes with consolidated registers', () => {
  const notes = parseFinalNotes(
    `OVERVIEW| The team selected Postgres and assigned the migration.
SECTION| Database choice
STATUS| discussed
SUMMARY| Relational data and future flexibility drove the choice.
DECISION| Use Postgres for the user database.
ACTION| Sarah — prepare the migration script — Thursday
QUESTION| Will connection pooling reduce p99 latency?`,
    'Database choice',
  );
  const markdown = finalNotesToMarkdown(notes, {
    title: 'Architecture Review',
    date: 'Jul 27, 2026',
    duration: '32:10',
  });

  assert.match(markdown, /^# Architecture Review/);
  assert.match(markdown, /> The team selected Postgres/);
  assert.match(markdown, /### 1\. Database choice/);
  assert.match(markdown, /## Decisions\n- Use Postgres/);
  assert.match(markdown, /## Action Items\n- \[ \] Sarah/);
  assert.match(markdown, /## Open Questions/);
  assert.equal(finalNotesHasSubstance(notes), true);
  assert.deepEqual(finalNotesCounts(notes), {
    sections: 1,
    decisions: 1,
    actions: 1,
    keypoints: 0,
    questions: 1,
  });
});

test('markdown salvage accepts ordinary model headings and bullets', () => {
  const notes = parseFinalNotes(`## Budget
- **Summary:** The cap stayed at seven hundred fifty thousand dollars.
- **Decision:** Keep the existing annual cap.
- **Action:** Finance — publish the split — Friday`);
  assert.equal(notes.sections[0].title, 'Budget');
  assert.equal(notes.sections[0].decisions[0], 'Keep the existing annual cap.');
  assert.equal(notes.sections[0].actions[0], 'Finance — publish the split — Friday');
});

test('overview-only output is not promoted as final meeting notes', () => {
  const notes = parseFinalNotes('OVERVIEW| The team discussed several important topics and possible next steps.');
  assert.equal(finalNotesHasSubstance(notes), false);
});

test('inline protocol fields from the small model parse independently', () => {
  const notes = parseFinalNotes(`OVERVIEW| 2 sentences explaining the meeting's purpose and outcome. The team kept the budget cap.
SECTION| Budget approval| STATUS| discussed| SUMMARY| The annual cap remains $750,000.| DECISION| Keep the existing cap.
SECTION| Accessibility review| STATUS| discussed| SUMMARY| Accessibility is pending.`,
  'Budget approval\nAccessibility review',
  'The annual budget cap remains $750,000. We agreed to keep it.');
  assert.equal(notes.overview, 'The team kept the budget cap.');
  assert.equal(notes.sections[0].status, 'discussed');
  assert.equal(notes.sections[0].summary, 'The annual cap remains $750,000.');
  assert.equal(notes.sections[0].decisions[0], 'Keep the existing cap.');
  assert.equal(notes.sections[1].status, 'not discussed');
  assert.equal(notes.sections[1].summary, '');
});

test('ordinary Markdown supplies the model summary and topic framing', () => {
  const notes = parseNaturalFinalNotes(`- **Summary**: The team chose Postgres and assigned the migration.
- **Topics**: Database choice, migration script, latency, OAuth
- **Decisions**: Use Postgres.
- **Action items**: Sarah owns the migration.`);
  assert.equal(notes.overview, 'The team chose Postgres and assigned the migration.');
  assert.deepEqual(
    notes.sections.map((section) => section.title),
    ['Database choice', 'migration script', 'latency', 'OAuth'],
  );
});

test('source register preserves exact decisions actions metrics and uncertainty', () => {
  const evidence = extractGroundedEvidence([
    'We will use Postgres for the user database.',
    'Sarah will own the migration script and have it ready by Thursday.',
    'Read latency hit 800 milliseconds at p99 last month.',
    'We still need to learn whether connection pooling will help.',
    'Someone should investigate monitoring, but no owner was assigned.',
  ]);
  assert.deepEqual(
    evidence.map((note) => note.cat),
    ['decisions', 'actions', 'keypoints', 'questions', 'keypoints'],
  );
  assert.match(evidence[1].text, /Sarah.*Thursday/);
  assert.match(evidence[2].text, /800.*p99/);
});

test('placeholder owners and deadlines never become action items', () => {
  const notes = parseFinalNotes(`OVERVIEW| The team left monitoring ownership open.
SECTION| Monitoring
SUMMARY| Better monitoring needs investigation.
ACTION| Owner: [Name] | Task: [Action] | Deadline: [Date]`);
  assert.deepEqual(notes.sections[0].actions, []);
});

test('mapped evidence restores exact registers lost by the reducer', () => {
  const source = [
    'The annual budget cap remains $750,000.',
    'We agreed to keep the existing cap.',
    'Priya will publish the rollout plan by Tuesday.',
  ];
  const base = parseFinalNotes(`OVERVIEW| The team kept its budget and planned the rollout.
SECTION| Budget approval
SUMMARY| The existing cap remains in place.
SECTION| Launch timing
SUMMARY| The rollout plan will be updated.`,
  'Budget approval\nLaunch timing\nAccessibility review',
  source);
  const evidence = parseEvidenceOutput(`TOPIC| Budget
DECISION| Keep the existing budget cap of $750,000.
ACTION| Priya will publish the rollout plan by Tuesday.`);
  const notes = mergeEvidenceIntoFinalNotes(
    base,
    evidence,
    'Budget approval\nLaunch timing\nAccessibility review',
    source,
  );
  assert.equal(notes.sections[0].decisions[0], 'Keep the existing budget cap of $750,000.');
  assert.equal(notes.sections[1].actions[0], 'Priya will publish the rollout plan by Tuesday.');
  assert.equal(notes.sections[2].status, 'not discussed');
});

test('explicitly separate evidence is kept in Other discussion', () => {
  const source = [
    'The team agreed to move the launch to August 15.',
    'Priya will publish the revised rollout plan by Tuesday.',
    'Separately, support will draft an escalation guide for the rollout.',
  ];
  const base = parseFinalNotes(`OVERVIEW| The launch plan was updated.
SECTION| Launch timing
SUMMARY| The launch moved and support work was assigned.`,
  'Launch timing',
  source);
  const evidence = parseEvidenceOutput(`TOPIC| Launch
DECISION| Move the launch to August 15.
ACTION| Priya will publish the revised rollout plan by Tuesday.
ACTION| Support will draft an escalation guide for the rollout.`);
  const notes = mergeEvidenceIntoFinalNotes(base, evidence, 'Launch timing', source);
  assert.deepEqual(
    notes.sections.map((section) => section.title),
    ['Launch timing', 'Other discussion'],
  );
  assert.equal(notes.sections[0].actions[0], 'Priya will publish the revised rollout plan by Tuesday.');
  assert.equal(notes.sections[1].actions[0], 'Support will draft an escalation guide for the rollout.');
});

test('unsupported reducer summaries do not override grounded mapped evidence', () => {
  const source = [
    'The vendor logs needed for root-cause analysis are still missing.',
    'We do not yet know whether the database or the payment gateway caused the slowdown.',
  ];
  const base = parseFinalNotes(`OVERVIEW| The payment gateway caused the slowdown.
SECTION| Incident findings
SUMMARY| The payment gateway was confirmed as the cause.`,
  'Incident findings',
  source);
  const evidence = parseEvidenceOutput(`TOPIC| Incident findings
KEYPOINT| The vendor logs needed for root-cause analysis are still missing.
QUESTION| We do not yet know whether the database or the payment gateway caused the slowdown.`);
  const notes = mergeEvidenceIntoFinalNotes(base, evidence, 'Incident findings', source);
  const markdown = finalNotesToMarkdown(notes, { title: 'Incident Follow-up' });
  assert.doesNotMatch(markdown, /confirmed as the cause/i);
  assert.doesNotMatch(notes.overview, /^The payment gateway caused/i);
  assert.match(markdown, /do not yet know whether/i);
});

test('positive evidence contradicted by transcript negation is rejected', () => {
  const source = 'The OAuth provider has not sent sandbox credentials, so auth remains blocked.';
  const evidence = parseEvidenceOutput(`TOPIC| Auth blocker
ACTION| The OAuth provider has sent sandbox credentials.`);
  const notes = mergeEvidenceIntoFinalNotes(
    { overview: 'Authentication remains blocked.', sections: [] },
    evidence,
    '',
    source,
  );
  assert.equal(notes.sections.length, 0);
  assert.equal(finalNotesHasSubstance(notes), false);
});
