/**
 * qwen_reference.mjs — DOM-free reference generator for the Qwen final-notes policy.
 *
 * Captures the CURRENT behavior of the Qwen final-notes pipeline as it ships in
 * index.html (on the `rust-refactor` HEAD this port is taken from), so the Rust
 * port in `crates/silent-notes/src/qwen.rs` can be proven byte-identical.
 *
 * The functions below are COPIED VERBATIM from index.html — no edits, no
 * "cleanups" — so the golden fixtures are an honest record of shipping behavior.
 * The only thing removed is the DOM/model glue (the `generateFinalNotes` method
 * reads the transcript out of `.transcript-text` elements and posts chunks to a
 * Web Worker); we reproduce the *pure* parts of that method as
 * `finalNotesPipeline(transcript)` so the chunk targeting + cap + dedup ordering
 * are all captured. The model call itself is not pure and is out of scope — the
 * Rust policy emits the chunks/system-prompt as typed commands; the worker runs
 * the model; the Rust policy parses + dedups the returned text. This reference
 * therefore models the model output as an injectable per-chunk function so we can
 * golden the chunking → parse → dedup deterministically.
 *
 * Source anchors (index.html on rust-refactor):
 *   - NOTES_SYSTEM, NOTE_CAT_MAP                 ~2409-2430
 *   - parseQwenNotes                             ~2432-2449
 *   - chunkTranscript                            ~2451-2463
 *   - _noteKw / dedupeNotes                      ~2465-2485
 *   - OPENQ_STOP (stopword set, reused by dedup) ~2494
 *   - generateFinalNotes chunk-target + .slice(0,22) + dedupeNotes ~4854-4878
 *
 * Run: node qwen_reference.mjs   (writes ../golden_qwen.json)
 */

// ── VERBATIM from index.html ────────────────────────────────────────────────

const NOTE_CAT_MAP = { DECISION: 'decisions', ACTION: 'actions', KEYPOINT: 'keypoints', QUESTION: 'questions' };

/** Parse Qwen note output. Accepts "TAG| text", "TAG: text", or "TAG - text"; drops NONE/empties.
    TOPIC lines set the outline heading carried by every following note from the same chunk. */
function parseQwenNotes(raw) {
  const out = [];
  let topic = null;
  for (let line of String(raw || '').replace(/<think>[\s\S]*?<\/think>/g, '').split('\n')) {
    line = line.trim();
    if (!line || /^none\.?$/i.test(line)) continue;
    const m = line.match(/^[-*\s]*(TOPIC|DECISION|ACTION|KEYPOINT|QUESTION)\s*[|:\-–]\s*(.+)$/i);
    if (!m) continue;
    let text = m[2].replace(/^["'\s]+|["'\s]+$/g, '').trim();
    if (text.length < 4 || /^none\.?$/i.test(text)) continue;
    if (m[1].toUpperCase() === 'TOPIC') { topic = text.length > 60 ? text.slice(0, 59).trim() + '…' : text; continue; }
    if (text.length > 160) text = text.slice(0, 159).trim() + '…';
    out.push({ cat: NOTE_CAT_MAP[m[1].toUpperCase()], text, topic });
  }
  return out;
}

/** Split a transcript into ~target-char chunks on sentence boundaries (keeps context coherent). */
function chunkTranscript(text, target = 1100) {
  const sentences = String(text).match(/[^.!?]+[.!?]+|\S[^.!?]*$/g) || [String(text)];
  const chunks = [];
  let cur = '';
  for (const s of sentences) {
    const seg = s.trim(); if (!seg) continue;
    if (cur && (cur.length + seg.length + 1) > target) { chunks.push(cur); cur = ''; }
    cur += (cur ? ' ' : '') + seg;
  }
  if (cur) chunks.push(cur);
  return chunks;
}

const OPENQ_STOP = new Set(('the a an and or but if then of to in on at for with from by is are was were be been being this that these those we you they it i he she them us our your their will would can could should do does did not as so about into over under more most very just also out up down off than too what how why when where who which whats hows').split(' '));

/** Content-word set for dedup (reuses the open-question stopword list). */
function _noteKw(t) {
  return new Set((String(t).toLowerCase().match(/\b[a-z][a-z']{2,}\b/g) || []).filter(w => !OPENQ_STOP.has(w)));
}
/** Drop near-duplicate notes (same category, ≥60% keyword overlap) from overlapping chunks. */
function dedupeNotes(notes) {
  const kept = [];
  for (const n of notes) {
    const kw = _noteKw(n.text);
    let dup = false;
    for (const k of kept) {
      if (k.cat !== n.cat) continue;
      const kkw = _noteKw(k.text);
      if (!kw.size || !kkw.size) { if (k.text.toLowerCase() === n.text.toLowerCase()) { dup = true; break; } continue; }
      let inter = 0; for (const w of kw) if (kkw.has(w)) inter++;
      if (inter / Math.min(kw.size, kkw.size) >= 0.6) { dup = true; break; }
    }
    if (!dup) kept.push(n);
  }
  return kept;
}

// The chunk-target + cap, lifted from generateFinalNotes (index.html ~4866-4867):
//   const target = Math.max(500, Math.ceil(transcript.length / 18));
//   const chunks = chunkTranscript(transcript, target).slice(0, 22);
function finalNotesChunks(transcript) {
  const target = Math.max(500, Math.ceil(transcript.length / 18));
  return chunkTranscript(transcript, target).slice(0, 22);
}

// ── Golden fixture cases ────────────────────────────────────────────────────

import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// parseQwenNotes cases — exercise every branch: separators, NONE, TOPIC carry,
// length floors/caps, <think> stripping, leading bullets, em-dash separator.
const PARSE_CASES = {
  basic_pipe: 'DECISION| We will ship on Friday\nACTION| Bob will write the docs',
  colon_sep: 'DECISION: Adopt Rust\nKEYPOINT: latency dropped 40%',
  hyphen_sep: 'ACTION - Alice owns the migration',
  emdash_sep: 'QUESTION – who owns the rollout plan',
  topic_carry: 'TOPIC| Roadmap planning\nDECISION| Ship v2 in Q3\nACTION| draft the spec',
  topic_resets: 'TOPIC| First topic\nKEYPOINT| fact one\nTOPIC| Second topic\nKEYPOINT| fact two',
  none_dropped: 'NONE',
  none_with_period: 'none.',
  none_text_dropped: 'DECISION| none',
  too_short_dropped: 'ACTION| ok',
  bullet_prefix: '- DECISION| use the cache\n* ACTION| Carol reviews it',
  think_stripped: '<think>reasoning here\nmore reasoning</think>DECISION| ship it now',
  quotes_stripped: 'KEYPOINT| "revenue grew 3x"',
  topic_truncation: 'TOPIC| ' + 'A very long topic heading that runs well beyond sixty characters and should be truncated'
    + '\nKEYPOINT| a note that carries the truncated topic',
  text_truncation: 'KEYPOINT| ' + 'X'.repeat(200),
  lowercase_tags: 'decision| accepted the plan\naction| ship the build',
  unknown_tag_skipped: 'RANDOM| not a real tag\nDECISION| this one counts',
  blank_lines: '\n\nDECISION| keep going\n\n\nACTION| Dan ships\n',
  empty: '',
  whitespace_only: '   \n  \t ',
};

// chunkTranscript cases — sentence boundaries, target packing, no-punctuation
// fallback, whitespace segments.
const CHUNK_CASES = {
  single_sentence: { text: 'This is one sentence.', target: 1100 },
  two_short_pack: { text: 'First sentence here. Second sentence here.', target: 1100 },
  splits_on_target: { text: 'Alpha beta gamma. Delta epsilon zeta. Eta theta iota.', target: 25 },
  no_punctuation: { text: 'just a run on fragment with no terminal punctuation at all', target: 1100 },
  questions_and_bangs: { text: 'Are we ready? Yes we are! Then go.', target: 1100 },
  trailing_fragment: { text: 'Complete sentence. Dangling end without a period', target: 1100 },
  multi_space: { text: 'One.   Two.   Three.', target: 1100 },
  empty: { text: '', target: 1100 },
  default_target_500: { text: 'Sentence one is here. Sentence two follows.', target: 500 },
};

// dedupeNotes cases — overlap threshold, category isolation, empty-keyword path.
const DEDUP_CASES = {
  drops_high_overlap: [
    { cat: 'decisions', text: 'we will migrate the database to postgres', topic: null },
    { cat: 'decisions', text: 'migrate the database to postgres soon', topic: null },
  ],
  keeps_different_category: [
    { cat: 'decisions', text: 'migrate the database to postgres', topic: null },
    { cat: 'actions', text: 'migrate the database to postgres', topic: null },
  ],
  keeps_distinct: [
    { cat: 'keypoints', text: 'revenue grew forty percent', topic: null },
    { cat: 'keypoints', text: 'churn dropped to two percent', topic: null },
  ],
  empty_keyword_exact_dup: [
    { cat: 'questions', text: 'who?', topic: null },
    { cat: 'questions', text: 'who?', topic: null },
  ],
  empty_keyword_distinct: [
    { cat: 'questions', text: 'who?', topic: null },
    { cat: 'questions', text: 'why?', topic: null },
  ],
  order_preserved: [
    { cat: 'actions', text: 'alice ships the api gateway changes', topic: 't' },
    { cat: 'keypoints', text: 'the latency target is fifty milliseconds', topic: 't' },
    { cat: 'actions', text: 'alice ships the api gateway changes again', topic: 't' },
  ],
};

// finalNotesChunks cases — the target = max(500, ceil(len/18)) + .slice(0,22) cap.
const FINAL_CHUNK_CASES = {
  short_uses_500_floor: 'Short transcript. Just two sentences.',
};
// Build deterministic long transcripts.
{
  let long = '';
  for (let i = 1; i <= 60; i++) long += `Sentence number ${i} carries a substantive point worth recording for the record. `;
  FINAL_CHUNK_CASES.long_grows_target = long.trim();
  // Exercise the .slice(0, 22) cap: 30 sentences each ~280 chars, so two cannot
  // share a 500-char chunk (one sentence per chunk). Total length stays under
  // ~9000 so target = max(500, ceil(len/18)) stays at the 500 floor → 30 raw
  // chunks, capped to 22. (At the floor this is the only regime where the cap
  // fires; a longer transcript grows `target` and packs fewer, larger chunks.)
  let huge = '';
  for (let i = 1; i <= 30; i++) huge += `S${i} ` + 'z'.repeat(280) + '. ';
  FINAL_CHUNK_CASES.caps_at_22 = huge.trim();
}

// ── Emit golden ─────────────────────────────────────────────────────────────

const golden = {
  _meta: {
    source: 'index.html (rust-refactor HEAD) — Qwen final-notes policy',
    generator: 'crates/silent-notes/goldens/qwen/reference/qwen_reference.mjs',
    note: 'Functions copied verbatim from index.html. This is the parity contract for crates/silent-notes/src/qwen.rs.',
  },
  parse_qwen_notes: Object.fromEntries(
    Object.entries(PARSE_CASES).map(([k, raw]) => [k, { input: raw, output: parseQwenNotes(raw) }])
  ),
  chunk_transcript: Object.fromEntries(
    Object.entries(CHUNK_CASES).map(([k, { text, target }]) => [k, { text, target, output: chunkTranscript(text, target) }])
  ),
  dedupe_notes: Object.fromEntries(
    Object.entries(DEDUP_CASES).map(([k, notes]) => [k, { input: notes, output: dedupeNotes(notes) }])
  ),
  final_notes_chunks: Object.fromEntries(
    Object.entries(FINAL_CHUNK_CASES).map(([k, transcript]) => [k, {
      transcript_len: transcript.length,
      target: Math.max(500, Math.ceil(transcript.length / 18)),
      output: finalNotesChunks(transcript),
    }])
  ),
};

const outPath = join(__dirname, '..', 'golden_qwen.json');
writeFileSync(outPath, JSON.stringify(golden, null, 2) + '\n');
console.log('wrote', outPath);
console.log('parse cases:', Object.keys(golden.parse_qwen_notes).length);
console.log('chunk cases:', Object.keys(golden.chunk_transcript).length);
console.log('dedup cases:', Object.keys(golden.dedupe_notes).length);
console.log('final-chunk cases:', Object.keys(golden.final_notes_chunks).length);
