/**
 * questions_reference.mjs — DOM-free reference generator for the smart-question
 * scheduling policy.
 *
 * Captures the CURRENT behavior of the SmartQ teleprompter controller and the
 * stop-time question recap as they ship in index.html, so the Rust port in
 * `crates/silent-notes/src/questions.rs` can be proven equivalent.
 *
 * SmartQ in index.html mixes policy (timing gates, window accumulation, type
 * rotation, dedup ring) with DOM rendering and an async model call. This
 * reference re-expresses the PURE policy as deterministic functions, holding the
 * two non-pure inputs explicit:
 *   - clock time  → passed in as `now` (ms), instead of Date.now()
 *   - model output→ passed in as a scripted list of question strings, instead of
 *                   awaiting sharedQuestionGenerator.generate()
 *
 * Everything else (the gating arithmetic, the WINDOW_CHARS slice, the _typeIdx
 * rotation, the _recent ring of 8 with _norm dedup, the two-attempt retry, the
 * badge "has-new when minimized" state, the recap split/dedup/slice-3) is copied
 * from the SmartQ object so the captured behavior is honest.
 *
 * Source anchors (index.html on rust-refactor):
 *   - SmartQ constants/types/HEURISTICS  ~2188-2207
 *   - accumulate / _maybeGenerate        ~2218-2238
 *   - _generate (rotation + dedup retry) ~2239-2264
 *   - _norm                              ~2265
 *   - _render badge "has-new"            ~2275-2283 (minimized → add has-new)
 *   - reroll (expand + regenerate)       ~2291-2295
 *   - generateQuestionRecap split/dedup  ~4824-4847 (split→clean→dedup→slice(0,3))
 *
 * Run: node questions_reference.mjs   (writes ../golden_questions.json)
 */

// ── Policy constants, VERBATIM from SmartQ ──────────────────────────────────
const MIN_INTERVAL = 60000;   // ms between auto-generations
const MIN_CHARS = 220;        // new transcript chars before a fresh auto-question
const WINDOW_CHARS = 1200;    // rolling context cap

const TYPE_KEYS = ['clarify', 'risk', 'followup', 'coverage', 'deepen'];

// _norm — VERBATIM
function _norm(q) { return String(q || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim(); }

// ── A faithful pure re-implementation of SmartQ's stateful scheduler ────────
// Models the same fields and the same gate/rotation/dedup logic, but time is an
// argument and the model is a scripted reply queue. Records, per step, exactly
// what SmartQ would do (generate or not, which type, which question rendered,
// badge state) so the golden is a behavioral trace.
class SmartQRef {
  constructor(enabledTypes = TYPE_KEYS) {
    this.enabledTypes = enabledTypes.length ? enabledTypes : TYPE_KEYS;
    this._win = '';
    this._lastGenAt = 0;
    this._charsSinceGen = 0;
    this._recent = [];
    this._typeIdx = 0;
    this._busy = false;     // in this synchronous reference a generate completes within the call
    this._gen = 0;
    // rendering/badge state (modelled, not DOM): minimized + has-new dot
    this.minimized = true;
    this.hasNew = false;
    this.shown = null;      // { question, typeLabel } currently rendered, or null
  }

  reset() {
    this._win = ''; this._lastGenAt = 0; this._charsSinceGen = 0;
    this._recent = []; this._typeIdx = 0; this._busy = false; this._gen++;
    this.shown = null; this.hasNew = false;
  }

  // SmartQ.accumulate gate logic (without prewarm side effects).
  // Returns true iff this accumulate would trigger a _generate.
  _wouldGenerate(now) {
    if (this._busy) return false;
    const enoughNew = this._charsSinceGen >= MIN_CHARS;
    const enoughTime = this._lastGenAt === 0 || (now - this._lastGenAt) >= MIN_INTERVAL;
    return this._win.length >= 120 && enoughNew && enoughTime;
  }

  // Accumulate text at time `now`. If the gate fires, runs a generate using the
  // scripted `replies` (the model output for up-to-two attempts). Returns a
  // trace record describing what happened.
  accumulate(text, now, replies) {
    const t = String(text).trim();
    if (!t) return { action: 'ignored_empty', win_len: this._win.length };
    this._win = (this._win + ' ' + t).slice(-WINDOW_CHARS);
    this._charsSinceGen += t.length;
    if (!this._wouldGenerate(now)) {
      return { action: 'accumulated', win_len: this._win.length, chars_since_gen: this._charsSinceGen };
    }
    return this._generate(now, replies, 'auto');
  }

  // SmartQ._generate: two attempts, rotating type each attempt; accept the first
  // unique question (not in _recent by _norm); fall back to the last attempt's
  // result. Pushes to _recent (ring of 8). Sets badge when minimized.
  _generate(now, replies, trigger) {
    if (this._win.length < 40) return { action: 'skipped_short_window', win_len: this._win.length };
    this._lastGenAt = now;
    this._charsSinceGen = 0;
    const attempts = [];
    let best = null;
    for (let attempt = 0; attempt < 2; attempt++) {
      const typeKey = this.enabledTypes[this._typeIdx++ % this.enabledTypes.length];
      const question = replies[attempt] !== undefined ? replies[attempt] : '';
      attempts.push({ attempt, type: typeKey, question });
      best = { question, type: typeKey };
      if (question && !this._recent.includes(_norm(question))) break;  // unique → use it
    }
    if (!best) return { action: 'no_result', trigger };
    this._recent.push(_norm(best.question));
    if (this._recent.length > 8) this._recent.shift();
    // _render: show question; if minimized, raise the new-question badge.
    this.shown = { question: best.question, type: best.type };
    if (this.minimized) this.hasNew = true;
    return {
      action: 'generated',
      trigger,
      attempts,
      chosen_type: best.type,
      chosen_question: best.question,
      recent_after: [...this._recent],
      type_idx_after: this._typeIdx,
      badge_has_new: this.hasNew,
      win_len: this._win.length,
    };
  }

  // SmartQ.reroll: if minimized, expand (clear badge); then force a generate
  // regardless of timing/char gates (this is the "Suggest another" button).
  reroll(now, replies) {
    let expanded = false;
    if (this.minimized) { this.minimized = false; this.hasNew = false; expanded = true; }
    const r = this._generate(now, replies, 'reroll');
    return { expanded, ...r };
  }

  // toggleSmartQ: flips minimized; expanding clears the badge.
  toggle() {
    this.minimized = !this.minimized;
    if (!this.minimized) this.hasNew = false;
    return { minimized: this.minimized, badge_has_new: this.hasNew };
  }
}

// ── Recap split/dedup/slice — VERBATIM from generateQuestionRecap ───────────
// For each enabled type, the model returns multi-line text; this cleans each
// line (strip leading numbering/bullets and surrounding quotes), drops lines
// length<=6, dedups within the group (Set), and keeps the first 3.
function recapCleanGroup(rawQuestion) {
  const qs = String(rawQuestion || '').split('\n')
    .map(x => x.replace(/^[\s\-\d.\)]+/, '').replace(/^["']|["']$/g, '').trim())
    .filter(x => x.length > 6);
  return [...new Set(qs)].slice(0, 3);
}

// ── Golden fixture cases ────────────────────────────────────────────────────
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
const __dirname = dirname(fileURLToPath(import.meta.url));

// Scheduling traces: drive a fresh SmartQRef through a scripted sequence of
// (text, now, replies) accumulate calls + the occasional reroll/toggle, and
// record the full trace. Each scenario is a named list of steps.
function runScenario(steps, enabledTypes) {
  const sq = new SmartQRef(enabledTypes);
  const trace = [];
  for (const step of steps) {
    if (step.op === 'accumulate') trace.push({ op: 'accumulate', text_len: step.text.length, now: step.now, result: sq.accumulate(step.text, step.now, step.replies || []) });
    else if (step.op === 'reroll') trace.push({ op: 'reroll', now: step.now, result: sq.reroll(step.now, step.replies || []) });
    else if (step.op === 'toggle') trace.push({ op: 'toggle', result: sq.toggle() });
    else if (step.op === 'reset') { sq.reset(); trace.push({ op: 'reset', result: { win_len: sq._win.length, type_idx: sq._typeIdx } }); }
  }
  return trace;
}

// 120-char filler chunk so the >=120 window gate and >=220 char gate are easy to reason about.
const C = (n) => 'word '.repeat(Math.ceil(n / 5)).slice(0, n);

const SCHED = {
  // Gate not met: window too short, no generation.
  short_window_no_gen: {
    types: TYPE_KEYS,
    steps: [{ op: 'accumulate', text: C(50), now: 1000, replies: ['Q1?'] }],
  },
  // First generation: window>=120, chars>=220, lastGenAt==0 so time gate open.
  first_generation: {
    types: TYPE_KEYS,
    steps: [{ op: 'accumulate', text: C(240), now: 5000, replies: ['What is the deadline?'] }],
  },
  // Type rotation across three generations (clarify → risk → followup),
  // each spaced > MIN_INTERVAL apart, each with enough new chars.
  type_rotation: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['Q1 unique?'] },
      { op: 'accumulate', text: C(240), now: 61000, replies: ['Q2 unique?'] },
      { op: 'accumulate', text: C(240), now: 122000, replies: ['Q3 unique?'] },
    ],
  },
  // Dedup retry: first attempt returns a question already in _recent → second
  // attempt (next type) is used. Seed _recent by generating "Repeat?" first.
  dedup_retry: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['Repeat question?'] },
      // Second gen: attempt 0 returns the same normalized question → rejected;
      // attempt 1 returns a fresh one → used (and type advances twice).
      { op: 'accumulate', text: C(240), now: 61000, replies: ['Repeat question?', 'Fresh question?'] },
    ],
  },
  // Both attempts duplicate → falls back to the last attempt's (duplicate) result.
  dedup_both_dupe: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['Only one?'] },
      { op: 'accumulate', text: C(240), now: 61000, replies: ['Only one?', 'Only one?'] },
    ],
  },
  // Time gate blocks a too-soon second generation even with enough chars.
  time_gate_blocks: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 1000, replies: ['First?'] },
      { op: 'accumulate', text: C(240), now: 30000, replies: ['Too soon?'] },  // 29s < 60s → blocked
    ],
  },
  // Char gate blocks: enough time but not enough new chars.
  char_gate_blocks: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['First?'] },
      { op: 'accumulate', text: C(100), now: 120000, replies: ['Not enough chars?'] },  // 100 < 220
    ],
  },
  // Window slicing: accumulate well over WINDOW_CHARS so _win is tail-capped.
  window_cap: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: 'A'.repeat(1500), now: 0, replies: ['Capped?'] },
    ],
  },
  // Reroll forces a generation regardless of gates, expands if minimized,
  // clears the badge.
  reroll_forces_gen: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['First?'] },  // sets shown + badge (minimized)
      { op: 'reroll', now: 5000, replies: ['Rerolled?'] },              // ignores time gate; expands
    ],
  },
  // Toggle clears the badge when expanding.
  toggle_clears_badge: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['First?'] },  // badge raised (minimized)
      { op: 'toggle' },   // expand → badge cleared
      { op: 'toggle' },   // minimize again → badge stays cleared (only set on render)
    ],
  },
  // Reset clears window + recent + type index.
  reset_clears: {
    types: TYPE_KEYS,
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['Q?'] },
      { op: 'reset' },
      { op: 'accumulate', text: C(50), now: 1000, replies: ['too short?'] },
    ],
  },
  // Custom enabled types subset: only clarify + risk rotate.
  subset_types: {
    types: ['clarify', 'risk'],
    steps: [
      { op: 'accumulate', text: C(240), now: 0, replies: ['Q1?'] },
      { op: 'accumulate', text: C(240), now: 61000, replies: ['Q2?'] },
      { op: 'accumulate', text: C(240), now: 122000, replies: ['Q3?'] },  // wraps back to clarify
    ],
  },
  // _recent ring eviction: generate 10 unique questions; _recent holds last 8.
  recent_ring_evicts: {
    types: TYPE_KEYS,
    steps: Array.from({ length: 10 }, (_, i) => ({
      op: 'accumulate', text: C(240), now: i * 61000, replies: [`Unique question number ${i}?`],
    })),
  },
};

// Recap cleaning cases.
const RECAP = {
  numbered_lines: '1. What is the deadline?\n2. Who owns the rollout?\n3. What does done look like?',
  bullets_and_quotes: '- "Is the budget approved?"\n* Will QA sign off?\n  Are we blocked on legal?',
  dedup_within_group: 'Same question here?\nSame question here?\nA different one?',
  drops_short_lines: 'ok?\nA real substantive question to keep?\nno',
  caps_at_three: 'Q one is long enough?\nQ two is long enough?\nQ three is long enough?\nQ four is long enough?',
  empty: '',
  whitespace: '   \n   ',
};

const golden = {
  _meta: {
    source: 'index.html (rust-refactor HEAD) — SmartQ scheduling + question recap',
    generator: 'crates/silent-notes/goldens/questions/reference/questions_reference.mjs',
    note: 'SmartQ policy re-expressed pure (clock + model output injected). Parity contract for crates/silent-notes/src/questions.rs.',
    constants: { MIN_INTERVAL, MIN_CHARS, WINDOW_CHARS, TYPE_KEYS },
  },
  norm: Object.fromEntries(
    ['Hello, World!', '  Spaces   and—dashes  ', 'MixedCase123', '???', ''].map(s => [JSON.stringify(s), { input: s, output: _norm(s) }])
  ),
  scheduling: Object.fromEntries(
    Object.entries(SCHED).map(([k, { steps, types }]) => [k, { enabled_types: types, trace: runScenario(steps, types) }])
  ),
  recap_clean: Object.fromEntries(
    Object.entries(RECAP).map(([k, raw]) => [k, { input: raw, output: recapCleanGroup(raw) }])
  ),
};

const outPath = join(__dirname, '..', 'golden_questions.json');
writeFileSync(outPath, JSON.stringify(golden, null, 2) + '\n');
console.log('wrote', outPath);
console.log('scheduling scenarios:', Object.keys(golden.scheduling).length);
console.log('recap cases:', Object.keys(golden.recap_clean).length);
console.log('norm cases:', Object.keys(golden.norm).length);
