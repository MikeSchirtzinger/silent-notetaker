/**
 * Whole-meeting notes policy.
 *
 * The live outline is deliberately provisional. At Stop, the local model reads
 * the complete meeting source in one natural meeting-notes pass. A deterministic
 * source register then preserves exact decisions, assignments, metrics,
 * blockers, and unresolved questions while rejecting unsupported model prose.
 *
 * This module is DOM-free so the same policy is used by the browser, native
 * tests, and the notes-quality benchmark.
 */

export const DIRECT_TRANSCRIPT_MAX_CHARS = 24000;
export const MAX_AGENDA_ITEMS = 12;
export const MAX_AGENDA_ITEM_CHARS = 160;
export const MAX_REDUCER_EVIDENCE_CHARS = 14000;

const FINAL_TAGS = new Set([
  'OVERVIEW',
  'SECTION',
  'STATUS',
  'SUMMARY',
  'DECISION',
  'ACTION',
  'KEYPOINT',
  'QUESTION',
]);

const CATEGORY_KEYS = {
  DECISION: 'decisions',
  ACTION: 'actions',
  KEYPOINT: 'keypoints',
  QUESTION: 'questions',
};

const STATUS_VALUES = new Map([
  ['discussed', 'discussed'],
  ['complete', 'discussed'],
  ['covered', 'discussed'],
  ['partial', 'partial'],
  ['partially discussed', 'partial'],
  ['partially covered', 'partial'],
  ['not discussed', 'not discussed'],
  ['not covered', 'not discussed'],
  ['skipped', 'not discussed'],
]);

const STOPWORDS = new Set([
  'about', 'after', 'again', 'also', 'been', 'before', 'being', 'between',
  'could', 'does', 'from', 'have', 'into', 'just', 'more', 'most', 'other',
  'should', 'that', 'their', 'there', 'these', 'they', 'this', 'those',
  'through', 'under', 'very', 'what', 'when', 'where', 'which', 'while',
  'with', 'would', 'your',
]);

const GENERIC_AGENDA_WORDS = new Set([
  'agenda', 'approval', 'discussion', 'findings', 'followup', 'item',
  'overview', 'planning', 'review', 'status', 'timing', 'update',
]);

export const EVIDENCE_MAP_SYSTEM = `Extract precise meeting evidence from one transcript excerpt.

Start with TOPIC| and a short topic name. Then put every important item on its
own line using only these prefixes:
DECISION|
ACTION|
KEYPOINT|
QUESTION|

Capture exact names, owners, numbers, dates, deadlines, metrics, blockers, and
decisions. ACTION requires a named person or role. Preserve negative words such
as not, missing, blocked, waiting, and unknown exactly; never reverse them.
Copy the source wording instead of rewriting it. A suggestion without an owner
is KEYPOINT, not ACTION. Uncertainty is QUESTION, not DECISION. Never copy these
instructions or invent missing details. Skip filler. Output only tagged lines,
or NONE when there is no important evidence.`;

export const FINAL_NOTES_SYSTEM = `Write concise, accurate meeting notes using
the complete meeting source. Use only facts stated in the source. Write ordinary
Markdown with a short Summary, Topics, Decisions, Action items, and Open
questions. Preserve names, numbers, dates, metrics, blockers, negative
statements, and late-meeting details. Do not turn uncertainty into a conclusion.
Do not repeat the same fact in several places. Omit empty sections. Do not
explain the format or mention the transcript, evidence index, or instructions.`;

function cleanAgendaLine(line) {
  return String(line || '')
    .trim()
    .replace(/^[-*•]\s+/, '')
    .replace(/^\d{1,2}[.)]\s+/, '')
    .replace(/^\[[ xX]\]\s+/, '')
    .trim();
}

/**
 * Parse an agenda as ordered, human-authored headings.
 *
 * One non-empty line becomes one item. Duplicate headings are collapsed
 * case-insensitively, preserving the first spelling and order.
 */
export function parseAgenda(text) {
  const seen = new Set();
  const items = [];
  for (const raw of String(text || '').split(/\r?\n/)) {
    const item = cleanAgendaLine(raw).slice(0, MAX_AGENDA_ITEM_CHARS).trim();
    if (!item) continue;
    const key = normalizeHeading(item);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    items.push(item);
    if (items.length >= MAX_AGENDA_ITEMS) break;
  }
  return items;
}

function normalizeHeading(text) {
  return String(text || '')
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, ' ')
    .trim();
}

function cleanModelText(raw) {
  return String(raw || '')
    .replace(/<think>[\s\S]*?<\/think>/gi, '')
    .replace(/^```(?:text|markdown|md)?\s*/i, '')
    .replace(/\s*```$/i, '')
    .trim();
}

function cleanValue(value) {
  return String(value || '')
    .replace(/^\*+|\*+$/g, '')
    .replace(/^["']+|["']+$/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function parseTaggedLine(line) {
  const trimmed = String(line || '').trim();
  if (!trimmed) return null;

  const strict = trimmed.match(
    /^(OVERVIEW|SECTION|STATUS|SUMMARY|DECISION|ACTION|KEYPOINT|QUESTION)\s*[|:]\s*(.+)$/i,
  );
  if (strict) {
    return { tag: strict[1].toUpperCase(), value: cleanValue(strict[2]) };
  }

  // Salvage ordinary Markdown emitted by the small local model.
  const heading = trimmed.match(/^#{2,4}\s+(.+)$/);
  if (heading) return { tag: 'SECTION', value: cleanValue(heading[1]) };

  const bullet = trimmed
    .replace(/^[-*•]\s+/, '')
    .match(/^\*{0,2}(Overview|Status|Summary|Decision|Action|Key ?point|Question)\*{0,2}\s*[:—-]\s*(.+)$/i);
  if (!bullet) return null;
  return {
    tag: bullet[1].replace(/\s+/g, '').toUpperCase(),
    value: cleanValue(bullet[2]),
  };
}

function parseTaggedTokens(raw) {
  const text = cleanModelText(raw);
  const matcher = /\b(OVERVIEW|SECTION|STATUS|SUMMARY|DECISION|ACTION|KEYPOINT|QUESTION)\s*[|:]\s*/gi;
  const matches = [...text.matchAll(matcher)];
  if (!matches.length) return [];
  return matches.flatMap((match, index) => {
    const start = match.index + match[0].length;
    const end = index + 1 < matches.length ? matches[index + 1].index : text.length;
    const value = cleanValue(text.slice(start, end).replace(/^[|\s]+|[|\s]+$/g, ''));
    return value ? [{ tag: match[1].toUpperCase(), value }] : [];
  });
}

function cleanOverview(value) {
  const cleaned = cleanValue(value);
  const withoutInstruction = cleaned.replace(
    /^(?:(?:one|two|one or two|\d+)\s+)?sentences?\s+(?:explaining|summarizing|describing)[^.]*\.\s*/i,
    '',
  );
  return withoutInstruction || cleaned;
}

function newSection(title) {
  return {
    title: cleanValue(title) || 'Discussion',
    status: null,
    summary: '',
    decisions: [],
    actions: [],
    keypoints: [],
    questions: [],
  };
}

function sectionHasContent(section) {
  return Boolean(
    section.summary
    || section.decisions.length
    || section.actions.length
    || section.keypoints.length
    || section.questions.length,
  );
}

function normalizedStatus(value) {
  const v = cleanValue(value).toLowerCase();
  return STATUS_VALUES.get(v) || null;
}

function dedupeStrings(values) {
  const kept = [];
  const keptKeywords = [];
  for (const raw of values) {
    const value = cleanValue(raw);
    if (!value) continue;
    const keywords = new Set(
      (value.toLowerCase().match(/[\p{L}\p{N}']+/gu) || [])
        .filter((word) => word.length > 3 && !STOPWORDS.has(word)),
    );
    const duplicate = kept.some((prior, index) => {
      if (prior.toLowerCase() === value.toLowerCase()) return true;
      const priorKeywords = keptKeywords[index];
      if (!keywords.size || !priorKeywords.size) return false;
      let overlap = 0;
      for (const word of keywords) if (priorKeywords.has(word)) overlap += 1;
      return overlap / Math.min(keywords.size, priorKeywords.size) >= 0.72;
    });
    if (!duplicate) {
      kept.push(value);
      keptKeywords.push(keywords);
    }
  }
  return kept;
}

function isPlaceholderAction(value) {
  const text = cleanValue(value);
  return /\[(?:name|date|action|owner|deadline|task)\]/i.test(text)
    || /\b(?:owner|assignee)\b[^.!]{0,28}\b(?:not assigned|unassigned|unknown|tbd)\b/i.test(text)
    || /\bsomeone\b[^.!]{0,24}\b(?:should|will|needs? to)\b/i.test(text);
}

function contentWords(value) {
  return (normalizeHeading(value).split(' ') || [])
    .filter((word) => word.length > 3 && !STOPWORDS.has(word));
}

function normalizedWordSet(value) {
  return new Set(normalizeHeading(value).split(' ').filter(Boolean));
}

function evidenceIsSupported(value, sourceText) {
  const source = Array.isArray(sourceText) ? sourceText.join(' ') : String(sourceText || '');
  if (!source.trim()) return true;
  const sourceNormalized = normalizeHeading(source);
  const words = [...new Set(contentWords(value))];
  const overlapping = words.filter((word) => sourceNormalized.includes(word));
  if (words.length >= 2 && overlapping.length < 2) return false;

  const negation = /\b(?:no|not|never|without|missing|blocked|waiting|unknown|unassigned|cannot|can't|hasn't|haven't|didn't|doesn't|don't|won't)\b/i;
  if (!negation.test(value)) {
    for (const sentence of source.split(/(?<=[.!?])\s+|\n+/)) {
      if (!negation.test(sentence)) continue;
      const sentenceNormalized = normalizeHeading(sentence);
      const localOverlap = words.filter((word) => sentenceNormalized.includes(word)).length;
      if (localOverlap >= Math.max(2, Math.ceil(words.length * 0.45))) return false;
    }
  }
  return true;
}

function normalizedEvidenceCategory(note) {
  const text = cleanValue(note.text);
  const uncertainty = /\b(?:do not|don't|does not|doesn't|not yet)\s+know\b|\bwhether\b|\?$/i;
  const decision = /^(?:the\s+)?(?:approved|selected|scheduled|recommended)\b/i.test(text)
    || /\b(?:we|the team|team|group)\s+(?:agreed|decided|selected|approved|settled|recommended)\b/i.test(text)
    || /\b(?:will use|move(?:d)?\s+(?:the\s+)?(?:launch|rollout)|keep\s+(?:the\s+)?(?:existing\s+)?(?:budget\s+)?(?:cap|plan)|no release)\b/i.test(text);
  const assignment = text.match(
    /\b([\p{L}][\p{L}'-]{1,30})\s+(?:will|owns?|can own|is responsible for)\b/iu,
  );
  const excludedOwners = new Set([
    'anyone', 'everyone', 'group', 'it', 'nobody', 'release', 'someone',
    'somebody', 'team', 'they', 'this', 'we',
  ]);
  const assignedAction = Boolean(
    (assignment && !excludedOwners.has(assignment[1].toLowerCase()))
    || /^[\p{L}][\p{L}' -]{1,40}\s+[—–-]\s+/u.test(text),
  );

  if (uncertainty.test(text)) return 'questions';
  if (assignedAction) return 'actions';
  if (decision) return 'decisions';
  if (note.cat === 'actions') {
    if (isPlaceholderAction(text)) return 'keypoints';
    return 'keypoints';
  }
  if (note.cat === 'questions') return 'keypoints';
  if (note.cat === 'decisions' && !decision) return 'keypoints';
  return ['decisions', 'actions', 'keypoints', 'questions'].includes(note.cat)
    ? note.cat
    : 'keypoints';
}

function normalizeSection(section) {
  const out = {
    ...section,
    title: cleanValue(section.title) || 'Discussion',
    summary: cleanValue(section.summary),
  };
  for (const key of Object.values(CATEGORY_KEYS)) {
    out[key] = dedupeStrings(section[key] || []);
  }
  out.actions = out.actions.filter((action) => !isPlaceholderAction(action));
  if (!out.status) {
    out.status = sectionHasContent(out) ? 'discussed' : 'not discussed';
  }
  if (out.status === 'not discussed' && sectionHasContent(out)) {
    out.status = 'partial';
  }
  return out;
}

function sourceDiscussesAgenda(title, sourceText) {
  const source = new Set(normalizeHeading(
    Array.isArray(sourceText) ? sourceText.join(' ') : sourceText,
  ).split(' ').filter(Boolean));
  if (!source.size) return null;
  const allWords = agendaProbeWords(title);
  const distinctive = allWords.filter((word) => !GENERIC_AGENDA_WORDS.has(word));
  const probes = distinctive.length ? distinctive : allWords;
  return probes.length ? probes.some((word) => source.has(word)) : null;
}

function reconcileAgenda(parsedSections, agendaItems, sourceText = '') {
  if (!agendaItems.length) return parsedSections.map(normalizeSection);

  const unused = [...parsedSections];
  const ordered = agendaItems.map((agendaTitle) => {
    const agendaKey = normalizeHeading(agendaTitle);
    let matchIndex = unused.findIndex((section) => normalizeHeading(section.title) === agendaKey);
    if (matchIndex < 0) {
      matchIndex = unused.findIndex((section) => {
        const sectionKey = normalizeHeading(section.title);
        return sectionKey && agendaKey && (
          sectionKey.includes(agendaKey)
          || agendaKey.includes(sectionKey)
        );
      });
    }
    if (matchIndex < 0) {
      return { ...newSection(agendaTitle), status: 'not discussed' };
    }
    const [matched] = unused.splice(matchIndex, 1);
    if (sourceDiscussesAgenda(agendaTitle, sourceText) === false) {
      return { ...newSection(agendaTitle), status: 'not discussed' };
    }
    return normalizeSection({ ...matched, title: agendaTitle });
  });

  const offAgenda = unused.filter(sectionHasContent).map(normalizeSection);
  if (offAgenda.length) {
    const merged = newSection('Other discussion');
    const summaries = [];
    for (const section of offAgenda) {
      if (section.summary) summaries.push(`${section.title}: ${section.summary}`);
      for (const key of Object.values(CATEGORY_KEYS)) merged[key].push(...section[key]);
    }
    merged.summary = summaries.join(' ');
    merged.status = 'discussed';
    ordered.push(normalizeSection(merged));
  }
  return ordered;
}

/**
 * Parse the model's whole-meeting flat protocol into a stable structure.
 */
export function parseFinalNotes(raw, agendaText = '', sourceText = '') {
  const agenda = Array.isArray(agendaText) ? agendaText : parseAgenda(agendaText);
  const result = { overview: '', sections: [], agenda };
  let current = null;

  const cleanedRaw = cleanModelText(raw);
  let parsedTokens = /^#{2,4}\s+/m.test(cleanedRaw) ? [] : parseTaggedTokens(cleanedRaw);
  if (!parsedTokens.length) {
    parsedTokens = cleanedRaw
      .split(/\r?\n+/)
      .map(parseTaggedLine)
      .filter(Boolean);
  }

  for (const parsed of parsedTokens) {
    if (!parsed || !FINAL_TAGS.has(parsed.tag) || !parsed.value) continue;

    if (parsed.tag === 'OVERVIEW') {
      if (!result.overview) result.overview = cleanOverview(parsed.value);
      continue;
    }
    if (parsed.tag === 'SECTION') {
      current = newSection(parsed.value);
      result.sections.push(current);
      continue;
    }
    if (!current) {
      current = newSection(agenda[0] || 'Discussion');
      result.sections.push(current);
    }
    if (parsed.tag === 'STATUS') {
      current.status = normalizedStatus(parsed.value);
    } else if (parsed.tag === 'SUMMARY') {
      current.summary = current.summary
        ? `${current.summary} ${parsed.value}`
        : parsed.value;
    } else if (CATEGORY_KEYS[parsed.tag]) {
      current[CATEGORY_KEYS[parsed.tag]].push(parsed.value);
    }
  }

  result.sections = reconcileAgenda(result.sections, agenda, sourceText);
  if (!result.overview) {
    result.overview = result.sections
      .map((section) => section.summary)
      .filter(Boolean)
      .slice(0, 2)
      .join(' ');
  }
  return result;
}

function naturalLabel(line) {
  const cleaned = String(line || '')
    .trim()
    .replace(/^[-*•]\s*/, '')
    .replace(/^#{1,4}\s*/, '')
    .trim();
  const match = cleaned.match(
    /^\*{0,2}(Summary|Topics?|Discussion|Decisions?(?: made)?|Action items?|Open questions?)\*{0,2}\s*:?\s*(.*)$/i,
  );
  if (!match) return null;
  return {
    label: match[1].toLowerCase(),
    value: cleanValue(match[2]),
  };
}

function naturalTopics(value) {
  return String(value || '')
    .split(/[,;]|\s+\band\b\s+/i)
    .map((topic) => cleanValue(topic).replace(/[.]+$/, ''))
    .filter((topic) => topic.length >= 3 && topic.length <= 80)
    .slice(0, 6);
}

/**
 * Parse the small model's ordinary Markdown meeting notes.
 *
 * The model is deliberately not asked to emit a protocol: natural Markdown is
 * materially more coherent at 0.6B. Only its grounded summary and topic names
 * are consumed; exact registers are rebuilt from the source below.
 */
export function parseNaturalFinalNotes(raw, agendaText = '') {
  const agenda = Array.isArray(agendaText) ? agendaText : parseAgenda(agendaText);
  const text = cleanModelText(raw);
  const lines = text.split(/\r?\n/);
  let overview = '';
  let expectSummary = false;
  let inTopics = false;
  let inDiscussion = false;
  const topics = [];
  const overviewCandidates = [];

  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line) continue;
    const labeled = naturalLabel(line);
    if (labeled) {
      expectSummary = labeled.label === 'summary' && !labeled.value;
      inTopics = labeled.label.startsWith('topic');
      inDiscussion = labeled.label === 'discussion';
      if (labeled.label === 'summary' && labeled.value && !overview) {
        overview = labeled.value;
      } else if (labeled.label.startsWith('topic') && labeled.value) {
        topics.push(...naturalTopics(labeled.value));
      }
      continue;
    }

    if (expectSummary && !overview) {
      overview = cleanValue(line.replace(/^[-*•]\s*/, ''));
      expectSummary = false;
      continue;
    }
    if (inTopics) {
      topics.push(...naturalTopics(line.replace(/^[-*•]\s*/, '')));
      continue;
    }

    const heading = line.match(/^#{2,4}\s+(.+)$/);
    if (inDiscussion && heading) {
      const title = cleanValue(heading[1]);
      if (title && !naturalLabel(title)) topics.push(title);
    }

    const candidate = cleanValue(
      line.replace(/^[-*•]\s*/, '').replace(/\*+/g, ''),
    );
    if (candidate.length >= 24
        && !/^(?:meeting notes|decisions?|actions?|open questions?)$/i.test(candidate)) {
      overviewCandidates.push(candidate);
    }
  }

  const uniqueTopics = [];
  const seen = new Set();
  for (const topic of topics) {
    const key = normalizeHeading(topic);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    uniqueTopics.push(topic);
  }

  return {
    overview: overview || overviewCandidates[0] || '',
    sections: (agenda.length ? agenda : uniqueTopics).map((title) => newSection(title)),
    agenda,
  };
}

export function finalNotesHasSubstance(notes) {
  if (!notes || !Array.isArray(notes.sections)) return false;
  return notes.sections.some(sectionHasContent);
}

export function finalNotesCounts(notes) {
  const counts = { sections: 0, decisions: 0, actions: 0, keypoints: 0, questions: 0 };
  if (!notes || !Array.isArray(notes.sections)) return counts;
  counts.sections = notes.sections.length;
  for (const section of notes.sections) {
    for (const key of ['decisions', 'actions', 'keypoints', 'questions']) {
      counts[key] += (section[key] || []).length;
    }
  }
  return counts;
}

function agendaBlock(items) {
  if (!items.length) {
    return 'AGENDA: none supplied. Derive useful topic sections from the source.';
  }
  return `AGENDA (preserve this exact order):\n${items.map((item, index) => `${index + 1}. ${item}`).join('\n')}`;
}

function formatTranscriptSegments(segments) {
  if (Array.isArray(segments)) {
    return segments
      .map((segment, index) => {
        const text = cleanValue(typeof segment === 'string' ? segment : segment?.text);
        return text ? `[T${index + 1}] ${text}` : '';
      })
      .filter(Boolean)
      .join('\n');
  }
  return cleanModelText(segments);
}

/**
 * Build the source supplied to the whole-meeting synthesis call.
 *
 * Meetings within the local model's direct context use the complete transcript.
 * Longer meetings use the compact chronological source register.
 */
export function buildFinalNotesRequest({ transcript, transcriptSegments, agendaText, evidence = '' }) {
  const sourceInput = Array.isArray(transcriptSegments) && transcriptSegments.length
    ? transcriptSegments
    : transcript;
  const fullTranscript = formatTranscriptSegments(sourceInput);
  const transcriptLength = Array.isArray(sourceInput)
    ? sourceInput
      .map((segment) => cleanValue(typeof segment === 'string' ? segment : segment?.text))
      .filter(Boolean)
      .join(' ')
      .length
    : cleanModelText(sourceInput).length;
  const agenda = parseAgenda(agendaText);
  const direct = transcriptLength <= DIRECT_TRANSCRIPT_MAX_CHARS;

  let source;
  if (direct) {
    source = `FULL TRANSCRIPT:\n${fullTranscript}`;
    const compactEvidence = cleanModelText(evidence).slice(0, MAX_REDUCER_EVIDENCE_CHARS).trim();
    if (compactEvidence) {
      source += `\n\nEXTRACTED EVIDENCE INDEX:\n${compactEvidence}`;
    }
  } else {
    const compactEvidence = cleanModelText(evidence).slice(0, MAX_REDUCER_EVIDENCE_CHARS).trim();
    if (!compactEvidence) {
      throw new Error('Long meeting has no extracted evidence for final synthesis');
    }
    source = `CHRONOLOGICAL EVIDENCE FROM THE COMPLETE TRANSCRIPT:\n${compactEvidence}`;
  }

  return {
    agenda,
    mode: direct ? 'direct' : 'reduce',
    userText: `${agendaBlock(agenda)}\n\n${source}\n\nWrite the canonical meeting notes now.`,
  };
}

export function formatMappedEvidence(mapped) {
  return (mapped || [])
    .map((note) => {
      const part = Number(note.part) > 0 ? `E${Number(note.part)}` : 'E?';
      const topic = cleanValue(note.topic) || 'Discussion';
      const cat = String(note.cat || 'keypoints').toUpperCase();
      const text = cleanValue(note.text);
      return text ? `[${part}] ${topic} | ${cat} | ${text}` : '';
    })
    .filter(Boolean)
    .join('\n');
}

function cleanEvidenceValue(value) {
  return cleanValue(value)
    .replace(/^2\s*-\s*6\s+words?\s*:\s*/i, '')
    .replace(/^an explicit choice the group settled on\s*:\s*/i, '')
    .replace(/^a task assigned to a named person or role\s*:\s*/i, '')
    .replace(/^a specific fact, number, constraint, risk, or rationale\s*:\s*/i, '')
    .replace(/^an open or unresolved question\s*:\s*/i, '')
    .trim();
}

/**
 * Parse one extraction-map response into the same evidence shape used by the
 * browser host. The parser accepts both one-field-per-line and compact inline
 * output from the small local model.
 */
export function parseEvidenceOutput(raw, part = 0) {
  const text = cleanModelText(raw);
  const matcher = /\b(TOPIC|DECISION|ACTION|KEYPOINT|QUESTION)\s*[|:]\s*/gi;
  const matches = [...text.matchAll(matcher)];
  const notes = [];
  let topic = 'Discussion';
  for (let index = 0; index < matches.length; index += 1) {
    const match = matches[index];
    const start = match.index + match[0].length;
    const end = index + 1 < matches.length ? matches[index + 1].index : text.length;
    const value = cleanEvidenceValue(
      text.slice(start, end).replace(/^[|\s]+|[|\s]+$/g, ''),
    );
    if (!value || value.toLowerCase() === 'none') continue;
    const tag = match[1].toUpperCase();
    if (tag === 'TOPIC') {
      topic = value;
      continue;
    }
    notes.push({
      part: Number(part) || undefined,
      topic,
      cat: CATEGORY_KEYS[tag] || 'keypoints',
      text: value,
    });
  }
  return notes;
}

function agendaProbeWords(title) {
  const words = contentWords(title);
  const expansions = {
    accessibility: ['a11y'],
    approval: ['approved', 'cap'],
    auth: ['oauth', 'security'],
    budget: ['cap', 'finance'],
    database: ['postgres', 'mongo'],
    findings: ['cause', 'root'],
    incident: ['cause', 'failure', 'outage', 'slowdown'],
    launch: ['rollout'],
    monitoring: ['observability'],
    release: ['ship', 'shipping'],
    rollout: ['launch'],
    security: ['auth', 'oauth'],
  };
  return [...new Set(words.flatMap((word) => [word, ...(expansions[word] || [])]))];
}

function normalizeTopicTitle(value) {
  const words = cleanValue(value)
    .replace(/[|]+$/g, '')
    .split(/\s+/)
    .filter(Boolean);
  const seen = new Set();
  const unique = words.filter((word) => {
    const key = normalizeHeading(word);
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  let title = unique.join(' ').slice(0, 60).trim();
  if (title && title === title.toUpperCase() && /[A-Z]/.test(title)) {
    title = title.toLowerCase().replace(/\b\p{L}/gu, (letter) => letter.toUpperCase());
  } else if (title && title === title.toLowerCase()) {
    title = title.replace(/^\p{L}/u, (letter) => letter.toUpperCase());
  }
  return title || 'Discussion';
}

function cloneSection(section) {
  return {
    title: cleanValue(section.title) || 'Discussion',
    status: section.status || null,
    summary: cleanValue(section.summary),
    decisions: [...(section.decisions || [])],
    actions: [...(section.actions || [])],
    keypoints: [...(section.keypoints || [])],
    questions: [...(section.questions || [])],
  };
}

function sourceSegments(sourceText) {
  const values = Array.isArray(sourceText) ? sourceText : [sourceText];
  return values.flatMap((value) => String(value || '')
    .split(/(?<=[.!?])\s+|\n+/)
    .map(cleanValue)
    .filter(Boolean));
}

/**
 * Build an exact, source-backed register after the model has read the meeting.
 *
 * This is not a substitute response: the model still supplies the whole-meeting
 * summary and topic framing. The register is an always-on grounding boundary
 * that keeps explicit decisions, assignments, metrics, blockers, and unresolved
 * questions from being lost or rewritten by the small local model.
 */
export function extractGroundedEvidence(sourceText) {
  const excludedOwners = new Set([
    'anyone', 'everyone', 'group', 'it', 'nobody', 'release', 'someone',
    'somebody', 'team', 'they', 'this', 'we',
  ]);
  const decisionPattern = /\b(?:agreed|decided|approved|selected|settled|will use|keep the existing|no release will ship|move(?:d)? (?:the )?(?:launch|rollout))\b/i;
  const questionPattern = /\?|(?:do not|don't|does not|doesn't|not yet)\s+know\b|\bwhether\b|\bstill need to (?:learn|know|decide)\b|\b(?:open|unresolved) question\b/i;
  const denseFactPattern = /\b(?:because|blocked|missing|risk|confirmed|remains?|latency|p\d+|percent|metrics?|deadline|increase|decrease|reduced?|cut|faster|slower|understood|should investigate|should review)\b|\d/i;
  const notes = [];
  const seen = new Set();

  sourceSegments(sourceText).forEach((sentence, index) => {
    const text = cleanValue(sentence);
    if (text.length < 16) return;
    if (/^(?:hello|hi|hey|thanks|thank you|good morning|good afternoon)\b/i.test(text)) return;

    let cat = null;
    const assignment = text.match(
      /\b([\p{L}][\p{L}'-]{1,30})\s+(?:will|owns?|can own|is responsible for)\b/iu,
    );
    if (questionPattern.test(text)) {
      cat = 'questions';
    } else if (assignment && !excludedOwners.has(assignment[1].toLowerCase())) {
      cat = 'actions';
    } else if (decisionPattern.test(text)) {
      cat = 'decisions';
    } else if (denseFactPattern.test(text)) {
      cat = 'keypoints';
    }
    if (!cat) return;

    const key = normalizeHeading(text);
    if (!key || seen.has(key)) return;
    seen.add(key);
    const registerText = text
      .replace(/^(?:separately|off agenda|off-agenda|one more thing)[,:]?\s*/i, '')
      .replace(/^\p{Ll}/u, (letter) => letter.toUpperCase());
    notes.push({
      part: index + 1,
      topic: 'Discussion',
      cat,
      text: registerText,
    });
  });
  return notes;
}

function closestSourceSegment(noteText, sourceText) {
  const words = [...new Set(contentWords(noteText))];
  let best = '';
  let bestScore = 0;
  for (const segment of sourceSegments(sourceText)) {
    const normalized = normalizedWordSet(segment);
    const score = words.filter((word) => normalized.has(word)).length;
    if (score > bestScore) {
      best = segment;
      bestScore = score;
    }
  }
  return bestScore >= 2 ? best : '';
}

function evidenceMarkedSeparate(noteText, sourceText) {
  const segment = closestSourceSegment(noteText, sourceText);
  return /\b(?:separately|off agenda|off-agenda|unrelated|one more thing)\b/i.test(segment);
}

function canonicalEvidenceText(note, sourceText) {
  const noteText = cleanValue(note.text);
  const candidate = closestSourceSegment(noteText, sourceText);
  if (!candidate) return noteText;

  const source = normalizeHeading(
    Array.isArray(sourceText) ? sourceText.join(' ') : sourceText,
  );
  const words = [...new Set(contentWords(noteText))];
  const unknownWords = words.filter((word) => !source.includes(word));
  const allCaps = noteText === noteText.toUpperCase() && /\p{Lu}/u.test(noteText);
  const assigned = /\b[\p{L}][\p{L}'-]+\s+(?:will|owns?|can own|is responsible for)\b/iu;
  const sourceRestoresOwner = note.cat === 'actions'
    && !assigned.test(noteText)
    && assigned.test(candidate);
  const distorted = unknownWords.length >= 2
    || (words.length >= 4 && unknownWords.length / words.length >= 0.25);

  if (!allCaps && !sourceRestoresOwner && !distorted) return noteText;
  return cleanValue(candidate).replace(
    /^(?:separately|off agenda|off-agenda|one more thing)[,:]?\s*/i,
    '',
  );
}

function overviewLooksUsable(value, sourceText) {
  const overview = cleanValue(value);
  if (overview.length < 24) return false;
  if (/\b(?:OVERVIEW|SECTION|STATUS|SUMMARY|DECISION|ACTION|KEYPOINT|QUESTION)\s*[|:]/i.test(overview)) {
    return false;
  }
  if (/\b(?:2\s*-\s*6 words|meeting purpose|provided order|exact order|allowed line prefixes)\b/i.test(overview)) {
    return false;
  }
  if (/^\s*\d+[.)].*\b\d+[.)]/.test(overview)) return false;
  const source = normalizeHeading(Array.isArray(sourceText) ? sourceText.join(' ') : sourceText);
  const calendarWords = overview.toLowerCase().match(
    /\b(?:january|february|march|april|may|june|july|august|september|october|november|december|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b/g,
  ) || [];
  if (calendarWords.some((word) => !source.includes(word))) return false;
  return evidenceIsSupported(overview, sourceText);
}

function evidenceOverview(notes) {
  const priority = ['decisions', 'actions', 'keypoints', 'questions'];
  const items = [];
  for (const key of priority) {
    for (const section of notes) {
      for (const value of section[key] || []) {
        if (!items.some((item) => item.toLowerCase() === value.toLowerCase())) {
          items.push(value);
        }
        if (items.length >= 2) return items.join(' ');
      }
    }
  }
  return items.join(' ');
}

/**
 * Merge precise source-register evidence into the whole-meeting synthesis.
 *
 * The reducer may supply a grounded overview. The map owns agenda coverage and
 * the exact decision/action/fact/question register so the small model cannot
 * lose names, numbers, or late-meeting details during its final pass.
 */
export function mergeEvidenceIntoFinalNotes(
  baseNotes,
  evidenceNotes,
  agendaText = '',
  sourceText = '',
) {
  const agenda = Array.isArray(agendaText) ? agendaText : parseAgenda(agendaText);
  const baseSections = Array.isArray(baseNotes?.sections)
    ? baseNotes.sections.map(cloneSection)
    : [];
  const supported = (evidenceNotes || [])
    .filter((note) => note && cleanValue(note.text))
    .filter((note) => evidenceIsSupported(note.text, sourceText))
    .map((note) => ({
      ...note,
      topic: normalizeTopicTitle(note.topic),
      text: canonicalEvidenceText(note, sourceText),
    }))
    .map((note) => ({ ...note, cat: normalizedEvidenceCategory(note) }))
    .filter((note) => !/^(?:no|none|blocked|unknown|n\/?a|tbd)$/i.test(note.text))
    .filter((note) => note.cat !== 'keypoints' || contentWords(note.text).length >= 2);

  let sections;
  if (agenda.length) {
    sections = agenda.map((title) => {
      const section = newSection(title);
      section.status = sourceDiscussesAgenda(title, sourceText) === false
        ? 'not discussed'
        : 'partial';
      return section;
    });
    const ensureOther = () => {
      let other = sections.find((section) => normalizeHeading(section.title) === 'other discussion');
      if (!other) {
        other = newSection('Other discussion');
        other.status = 'discussed';
        sections.push(other);
      }
      return other;
    };

    for (const note of supported) {
      if (evidenceMarkedSeparate(note.text, sourceText)) {
        ensureOther()[note.cat].push(note.text);
        continue;
      }
      const topicWords = normalizedWordSet(note.topic);
      const noteWords = normalizedWordSet(note.text);
      let bestIndex = -1;
      let bestScore = 0;
      agenda.forEach((title, index) => {
        const probes = agendaProbeWords(title);
        const noteScore = probes.filter((word) => noteWords.has(word)).length;
        const topicScore = probes.filter((word) => topicWords.has(word)).length;
        const score = noteScore > 0 ? 100 + noteScore : topicScore;
        if (score > bestScore) {
          bestIndex = index;
          bestScore = score;
        }
      });
      const target = bestIndex >= 0 ? sections[bestIndex] : ensureOther();
      target[note.cat].push(note.text);
    }

    sections.slice(0, agenda.length).forEach((section, index) => {
      if (sourceDiscussesAgenda(agenda[index], sourceText) === false) {
        section.status = 'not discussed';
        return;
      }
      const registerCount = Object.values(CATEGORY_KEYS)
        .reduce((sum, key) => sum + section[key].length, 0);
      section.status = section.decisions.length
        || section.actions.length
        || registerCount >= 2
        ? 'discussed'
        : 'partial';
    });
  } else {
    sections = [];
    if (baseSections.length) {
      const seenTitles = new Set();
      sections = baseSections.flatMap((section) => {
        const title = normalizeTopicTitle(section.title);
        const key = normalizeHeading(title);
        if (!key || seenTitles.has(key)) return [];
        seenTitles.add(key);
        const clean = newSection(title);
        clean.status = 'discussed';
        return [clean];
      }).slice(0, 6);
      const ensureOther = () => {
        let other = sections.find((section) => normalizeHeading(section.title) === 'other discussion');
        if (!other) {
          other = newSection('Other discussion');
          other.status = 'discussed';
          sections.push(other);
        }
        return other;
      };
      for (const note of supported) {
        const noteWords = normalizedWordSet(note.text);
        let bestIndex = -1;
        let bestScore = 0;
        sections.forEach((section, index) => {
          const score = agendaProbeWords(section.title)
            .filter((word) => noteWords.has(word)).length;
          if (score > bestScore) {
            bestIndex = index;
            bestScore = score;
          }
        });
        const target = bestIndex >= 0 ? sections[bestIndex] : ensureOther();
        target[note.cat].push(note.text);
      }
      sections = sections.filter(sectionHasContent);
    } else {
      for (const note of supported) {
        const title = note.topic || 'Discussion';
        let section = sections.find(
          (candidate) => normalizeHeading(candidate.title) === normalizeHeading(title),
        );
        if (!section) {
          section = newSection(title);
          section.status = 'discussed';
          sections.push(section);
        }
        section[note.cat].push(note.text);
      }
    }
  }

  sections = sections.map(normalizeSection);
  const candidateOverview = cleanValue(baseNotes?.overview);
  const modelOverviewUsed = overviewLooksUsable(candidateOverview, sourceText);
  const overview = modelOverviewUsed
    ? candidateOverview
    : evidenceOverview(sections);
  return { overview, sections, agenda, modelOverviewUsed };
}

function markdownItem(label, text) {
  return `- **${label}:** ${cleanValue(text)}`;
}

/**
 * Render the canonical structure as ordinary shareable meeting-note Markdown.
 */
export function finalNotesToMarkdown(notes, meta = {}) {
  const title = cleanValue(meta.title) || 'Meeting Notes';
  const date = cleanValue(meta.date);
  const duration = cleanValue(meta.duration);
  const lines = [`# ${title}`];
  if (date || duration) {
    lines.push([date ? `**Date:** ${date}` : '', duration ? `**Duration:** ${duration}` : ''].filter(Boolean).join('  '));
  }
  if (cleanValue(notes?.overview)) {
    lines.push('', `> ${cleanValue(notes.overview)}`);
  }

  const sections = Array.isArray(notes?.sections) ? notes.sections : [];
  if (sections.length) lines.push('', '## Discussion');
  sections.forEach((section, index) => {
    lines.push('', `### ${index + 1}. ${cleanValue(section.title) || 'Discussion'}`);
    if (section.status && section.status !== 'discussed') {
      lines.push(`_${section.status === 'partial' ? 'Partially discussed' : 'Not discussed'}_`);
    }
    if (cleanValue(section.summary)) lines.push(cleanValue(section.summary));
    for (const text of section.decisions || []) lines.push(markdownItem('Decision', text));
    for (const text of section.actions || []) lines.push(markdownItem('Action', text));
    for (const text of section.keypoints || []) lines.push(markdownItem('Key point', text));
    for (const text of section.questions || []) lines.push(markdownItem('Open question', text));
  });

  const decisions = dedupeStrings(sections.flatMap((section) => section.decisions || []));
  const actions = dedupeStrings(sections.flatMap((section) => section.actions || []));
  const questions = dedupeStrings(sections.flatMap((section) => section.questions || []));

  if (decisions.length) {
    lines.push('', '## Decisions');
    for (const text of decisions) lines.push(`- ${text}`);
  }
  if (actions.length) {
    lines.push('', '## Action Items');
    for (const text of actions) lines.push(`- [ ] ${text}`);
  }
  if (questions.length) {
    lines.push('', '## Open Questions');
    for (const text of questions) lines.push(`- ${text}`);
  }
  return lines.join('\n').replace(/\n{3,}/g, '\n\n').trim();
}
