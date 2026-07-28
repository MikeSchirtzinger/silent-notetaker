import {
  finalNotesCounts,
  finalNotesHasSubstance,
} from '../../final-notes.mjs';

function normalized(text) {
  return String(text || '')
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^\p{L}\p{N}.]+/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function includesVariant(text, variants) {
  return variants.some((variant) => text.includes(normalized(variant)));
}

function conceptPresent(text, concept) {
  return (concept.all || []).every((variants) => includesVariant(text, variants));
}

function forbiddenClaimPresent(text, claim) {
  const normalizedClaim = normalized(claim);
  if (!text.includes(normalizedClaim)) return false;

  // A causal claim is not an invention when the notes explicitly preserve it
  // as unresolved. Score sentence-sized contexts so "we do not know whether X
  // caused Y" remains a grounded open question, while an asserted cause fails.
  if (!/\bcaused\b/.test(normalizedClaim)) return true;
  let cursor = 0;
  while (cursor < text.length) {
    const index = text.indexOf(normalizedClaim, cursor);
    if (index < 0) return false;
    const start = Math.max(0, text.lastIndexOf('.', index - 1) + 1);
    const nextPeriod = text.indexOf('.', index + normalizedClaim.length);
    const end = nextPeriod < 0 ? text.length : nextPeriod + 1;
    const context = text.slice(start, end);
    if (!/\b(?:whether|unknown|unclear|do not know|not yet know|open question)\b/.test(context)) {
      return true;
    }
    cursor = index + normalizedClaim.length;
  }
  return false;
}

function unsupportedSpecifics(fixture, notes) {
  const noteText = JSON.stringify(notes || {});
  const allowedText = normalized([
    ...(fixture.transcriptSegments || []),
    JSON.stringify(fixture.expectedConcepts || []),
  ].join(' '));
  const unsupported = new Set();

  const calendarWords = noteText.toLowerCase().match(
    /\b(?:january|february|march|april|may|june|july|august|september|october|november|december|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b/g,
  ) || [];
  calendarWords.forEach((word) => {
    if (!allowedText.includes(word)) unsupported.add(word);
  });

  const numbers = noteText.match(/\b\d+(?:[,.]\d+)*(?:st|nd|rd|th)?\b/gi) || [];
  numbers.forEach((value) => {
    const probe = normalized(value);
    const zeroGrouped = value.match(/^(\d{1,3})(?:,000)+$/);
    const supported = probe && (
      allowedText.includes(probe)
      || (zeroGrouped && allowedText.includes(zeroGrouped[1]))
    );
    if (!supported) unsupported.add(value.toLowerCase());
  });
  return [...unsupported];
}

function expectedAgendaScore(fixture, notes) {
  const expected = fixture.expectedAgenda;
  if (!Array.isArray(expected) || expected.length === 0) {
    return (notes.sections || []).length >= (fixture.expectedMinimums?.sections || 1) ? 1 : 0;
  }
  let matched = 0;
  expected.forEach((want, index) => {
    const got = notes.sections?.[index];
    if (
      got
      && normalized(got.title) === normalized(want.title)
      && got.status === want.status
    ) matched += 1;
  });
  return matched / expected.length;
}

function structureScore(fixture, notes, markdown) {
  if (!finalNotesHasSubstance(notes)) return 0;
  const counts = finalNotesCounts(notes);
  const minimums = fixture.expectedMinimums || {};
  const checks = [
    String(notes.overview || '').trim().length >= 24,
    counts.sections >= (minimums.sections || 1),
    counts.decisions >= (minimums.decisions || 0),
    counts.actions >= (minimums.actions || 0),
    counts.questions >= (minimums.questions || 0),
    String(markdown || '').length >= 180 && String(markdown || '').length <= 6000,
  ];
  return checks.filter(Boolean).length / checks.length;
}

export function scoreMeetingNotes(fixture, notes, markdown) {
  const corpus = normalized(markdown);
  const concepts = fixture.expectedConcepts || [];
  const conceptResults = concepts.map((concept) => ({
    id: concept.id,
    pass: conceptPresent(corpus, concept),
  }));
  const forbiddenResults = (fixture.forbiddenClaims || []).map((claim) => ({
    claim,
    present: forbiddenClaimPresent(corpus, claim),
  }));
  const unsupported = unsupportedSpecifics(fixture, notes);

  const coverage = concepts.length
    ? conceptResults.filter((result) => result.pass).length / concepts.length
    : 1;
  const grounding = forbiddenResults.some((result) => result.present) || unsupported.length
    ? 0
    : 1;
  const structure = structureScore(fixture, notes, markdown);
  const agenda = expectedAgendaScore(fixture, notes);
  const score = coverage * 0.5 + grounding * 0.2 + structure * 0.15 + agenda * 0.15;
  const agendaGate = !Array.isArray(fixture.expectedAgenda)
    || fixture.expectedAgenda.length === 0
    || agenda === 1;

  return {
    score: Number(score.toFixed(4)),
    pass: grounding === 1
      && coverage >= 0.75
      && structure >= 0.8
      && agendaGate
      && score >= 0.7,
    dimensions: {
      coverage: Number(coverage.toFixed(4)),
      grounding,
      structure: Number(structure.toFixed(4)),
      agenda: Number(agenda.toFixed(4)),
    },
    conceptResults,
    forbiddenResults,
    unsupportedSpecifics: unsupported,
  };
}
