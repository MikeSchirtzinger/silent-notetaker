// Reference NoteExtractor — a faithful, DOM-free port of the JS note-trigger
// policy in index.html. This is the BEHAVIOR CONTRACT the Rust port
// (crates/silent-notes/src/extractor.rs) must reproduce BYTE-IDENTICALLY.
//
// Run: node notes_ref.mjs  → writes ../extractor/*.json
//
// What is ported, verbatim from index.html (on hn-prep / rust-refactor):
//   - class NoteEngine            (~lines 2555-2679): triggers, analyze(), flush()
//   - const OpenQs                (~lines 2495-2531): open-question tracking
//   - const OPENQ_STOP            (~line  2494): stopword set
//   - the live-counter derivation (~lines 4340-4414, 2526-2530):
//       decisions/actions/keypoints count = number of notes in that category;
//       questions count = number of UNRESOLVED open questions
//       (OpenQs._updateCount overwrites the DOM section count for questions).
//
// The trigger regexes below are COPIED CHARACTER-FOR-CHARACTER from index.html
// so that `triggerPhrase` (= the JS `pattern.source`) is byte-identical in the
// goldens. Do NOT "tidy" them — the Rust port asserts equality against these
// exact source strings.
//
// DOM coupling is removed faithfully:
//   - NoteEngine.analyze/flush touch NO DOM — ported as-is.
//   - OpenQs._mark / _updateCount / openTexts read/write the DOM in the app;
//     here that state is held in memory (resolved flag + stored question text).
//     The observable result (which questions are open, the open count, and the
//     open-question texts) is identical to what the DOM reflected.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// =====================================================================
// OPENQ_STOP — verbatim from index.html (~line 2494).
// =====================================================================
const OPENQ_STOP = new Set(
  (
    "the a an and or but if then of to in on at for with from by is are was were be been being this that these those we you they it i he she them us our your their will would can could should do does did not as so about into over under more most very just also out up down off than too what how why when where who which whats hows"
  ).split(" ")
);

// =====================================================================
// OpenQs — DOM-free port of index.html (~lines 2495-2531).
// In the app, `_mark`/`openTexts` use the DOM; here we hold the same state in
// memory: each item stores its text so openTexts() needs no DOM. The resolved
// flag and the open count are computed identically.
// =====================================================================
const OpenQs = {
  items: [], // { id, text, keywords:[], resolved:bool }
  reset() {
    this.items = [];
  },
  _kw(t) {
    return [
      ...new Set((String(t).toLowerCase().match(/\b[a-z][a-z']{2,}\b/g) || [])),
    ].filter((w) => !OPENQ_STOP.has(w));
  },
  add(id, text) {
    if (id == null) return;
    // `text` stored so the DOM-free openTexts() matches the app's DOM read.
    this.items.push({ id, text: String(text).trim(), keywords: this._kw(text), resolved: false });
  },
  consider(text) {
    if (!this.items.length || /\?\s*$/.test(String(text).trim())) return; // skip if the line is itself a question
    const kws = new Set(this._kw(text));
    if (!kws.size) return;
    for (const q of this.items) {
      if (q.resolved) continue;
      let overlap = 0;
      for (const k of q.keywords) if (kws.has(k)) overlap++;
      if (overlap >= 2) {
        q.resolved = true;
      }
    }
  },
  openTexts() {
    return this.items.filter((q) => !q.resolved).map((q) => q.text).filter(Boolean);
  },
  openCount() {
    return this.items.filter((q) => !q.resolved).length;
  },
};

// =====================================================================
// NoteEngine — verbatim from index.html (~lines 2555-2679). The trigger
// regexes are copied character-for-character; analyze()/flush() are unchanged
// (they were already DOM-free). `Date.now()` is replaced by a deterministic
// monotonic counter so fixtures are reproducible (the timestamp value is not
// part of the ported policy — the app only uses it for jump-to-transcript).
// =====================================================================
class NoteEngine {
  constructor() {
    this.triggers = {
      decisions: [
        /we('ve| have) decided to/i,
        /we('ll| will) move forward with/i,
        /the decision is/i,
        /we('re| are) going (with|to go with)/i,
        /let's go with/i,
        /\bagreed\b/i,
        /final (answer|decision|call)/i,
        /we('ll| will) use/i,
        /we chose/i,
        /going ahead with/i,
      ],
      actions: [
        /(\w+) (will|is going to|needs to|should) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)/i,
        /I('ll| will) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)/i,
        /you('ll| will) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)/i,
        /action item/i,
        /\btodo\b/i,
        /needs to be done/i,
        /follow up (on|with)/i,
        /assigned to/i,
        /take a look at/i,
        /by (next |this |end of )?(monday|tuesday|wednesday|thursday|friday|saturday|sunday|week|month|sprint|quarter|day)/i,
        /deadline/i,
        /due (date|by)/i,
      ],
      questions: [
        /\?$/,
        /does anyone (know|have|think)/i,
        /what (do|should|would|can|if|are|is) /i,
        /how (do|should|would|can|will|are|is) /i,
        /can we /i,
        /should we /i,
        /still (need|needs) to (be )?resolved/i,
        /open question/i,
        /wondering (if|whether|about)/i,
        /not sure (if|whether|about|how)/i,
        /anyone (know|have|think|seen)/i,
      ],
      // Key points are NOT the default — they require substantive triggers
      keypoints: [
        /\d+[\s]*(%|percent|x|times|minutes|hours|days|lines|files|MB|GB|ms|seconds)/i, // numbers/metrics
        /increased?|decreased?|improved?|reduced?|grew|dropped|rose|fell/i, // changes
        /the (key|main|important|critical|biggest|primary|core) (thing|point|issue|problem|takeaway|insight|finding)/i,
        /in (summary|conclusion|short)/i,
        /the (result|outcome|finding|data|evidence) (is|shows?|suggests?|indicates?)/i,
        /turns out/i,
        /the reason (is|was|for)/i,
        /because of/i,
        /this means/i,
        /the (problem|issue|challenge|risk|blocker|bottleneck) is/i,
        /\b(never|always|must|critical|essential|required|mandatory)\b/i,
        /highest.leverage|biggest.impact|most.important/i,
      ],
    };
    this.buffer = "";
  }

  analyze(text, now) {
    // Accumulate text; split on sentence boundaries
    this.buffer += " " + text;
    const sentenceRx = /[^.!?]*[.!?]+/g;
    const sentences = [];
    let match;
    let lastIndex = 0;

    while ((match = sentenceRx.exec(this.buffer)) !== null) {
      sentences.push(match[0].trim());
      lastIndex = sentenceRx.lastIndex;
    }

    // Keep remainder in buffer
    this.buffer = this.buffer.slice(lastIndex).trimStart();

    // If buffer has grown large without punctuation, flush it
    if (this.buffer.length > 300) {
      sentences.push(this.buffer.trim());
      this.buffer = "";
    }

    const results = [];
    for (const sentence of sentences) {
      if (sentence.trim().length < 8) continue;

      let category = null; // Default: NO note. Only create notes for triggered content.
      let matchedTrigger = null;

      for (const [cat, patterns] of Object.entries(this.triggers)) {
        for (const pattern of patterns) {
          if (pattern.test(sentence)) {
            category = cat;
            matchedTrigger = pattern.source;
            break;
          }
        }
        if (matchedTrigger) break;
      }

      // Skip sentences that don't trigger any category — they're just transcript
      if (!category) continue;

      results.push({
        category,
        text: sentence.trim(),
        triggerPhrase: matchedTrigger,
        timestamp: now(),
      });
    }
    return results;
  }

  flush(now) {
    // Force-flush remaining buffer at stop
    if (this.buffer.trim().length < 8) {
      this.buffer = "";
      return [];
    }
    const text = this.buffer.trim();
    this.buffer = "";
    return this.analyze(text + ".", now);
  }
}

// =====================================================================
// Live pipeline — the exact call sequence from index.html addTranscript()
// (~lines 4340-4350) and stop() (~lines 4081-4089), DOM-free.
//
// Per final transcript line, with trigger detection ON:
//   noteResults = noteEngine.analyze(text)
//   OpenQs.consider(text)                       // resolve answered questions
//   for each note:
//     id = next note id
//     (render)
//     if note.category === 'questions': OpenQs.add(id, note.text)
//
// At stop(): noteEngine.flush() → render each (NO OpenQs.add for flushed —
// matches index.html, which does not add flushed notes to OpenQs).
//
// Counters (Appendix A row 16 "+ live counters"):
//   decisions/actions/keypoints = running count of notes in that category
//   questions                   = OpenQs.openCount() (unresolved open questions)
// =====================================================================
function runFixture(description, lines) {
  const engine = new NoteEngine();
  OpenQs.reset();

  let nextId = 1;
  let tick = 1000; // deterministic monotonic "timestamp" stand-in
  const now = () => tick++;
  const catCounts = { decisions: 0, actions: 0, keypoints: 0 };
  const trace = [];

  for (const line of lines) {
    const noteResults = engine.analyze(line, now);
    OpenQs.consider(line);
    const emitted = [];
    for (const note of noteResults) {
      const id = nextId++;
      if (note.category === "questions") {
        OpenQs.add(id, note.text);
      } else {
        catCounts[note.category]++;
      }
      emitted.push({
        id,
        category: note.category,
        text: note.text,
        triggerPhrase: note.triggerPhrase,
        timestamp: note.timestamp,
      });
    }
    trace.push({
      op: "analyze",
      line,
      emitted,
      counters: {
        decisions: catCounts.decisions,
        actions: catCounts.actions,
        keypoints: catCounts.keypoints,
        questions: OpenQs.openCount(),
      },
    });
  }

  // stop() → flush remaining buffer (no OpenQs.add for flushed notes).
  const flushed = engine.flush(now);
  const flushedEmitted = [];
  for (const note of flushed) {
    const id = nextId++;
    if (note.category !== "questions") catCounts[note.category]++;
    flushedEmitted.push({
      id,
      category: note.category,
      text: note.text,
      triggerPhrase: note.triggerPhrase,
      timestamp: note.timestamp,
    });
  }
  trace.push({
    op: "flush",
    emitted: flushedEmitted,
    counters: {
      decisions: catCounts.decisions,
      actions: catCounts.actions,
      keypoints: catCounts.keypoints,
      questions: OpenQs.openCount(),
    },
  });

  return {
    description,
    lines,
    expected: {
      trace,
      finalCounters: {
        decisions: catCounts.decisions,
        actions: catCounts.actions,
        keypoints: catCounts.keypoints,
        questions: OpenQs.openCount(),
      },
      openQuestions: OpenQs.openTexts(),
    },
  };
}

// =====================================================================
// Fixtures — representative transcript lines exercising every category,
// every distinct subtlety, and the open-question resolution path.
// =====================================================================

// Fixture 1: one note per category, clean sentence boundaries.
const decisionsActionsKeypointsQuestions = runFixture(
  "one trigger per category, clean punctuation",
  [
    "After a long debate we have decided to ship on Friday.",
    "Alice will handle the deployment and rollback plan.",
    "The latency improved by 40% after the cache change.",
    "What should we do about the staging environment?",
  ]
);

// Fixture 2: open-question lifecycle — a question is asked, then answered by a
// later declarative line sharing >=2 content words (OpenQs.consider resolves it,
// the questions counter drops back to 0).
const openQuestionResolution = runFixture(
  "open question asked then resolved by a later declarative line",
  [
    "Does anyone know which database we should migrate to?",
    "We profiled it and the database migration to Postgres is the plan.",
  ]
);

// Fixture 3: multiple open questions, only some resolved.
const multipleOpenQuestions = runFixture(
  "two open questions, one resolved one still open",
  [
    "How should we handle authentication for the mobile app?",
    "What about the billing integration timeline?",
    "For authentication on mobile we will reuse the existing OAuth tokens.",
  ]
);

// Fixture 4: buffering across lines — a sentence split across two analyze() calls
// (no terminal punctuation on the first), then completed.
const crossLineBuffering = runFixture(
  "sentence buffered across two lines, then completed",
  ["we have decided to", " adopt the new design system next sprint."]
);

// Fixture 5: the 300-char no-punctuation flush path inside analyze().
const longUnpunctuatedFlush = runFixture(
  "long unpunctuated buffer forces the >300 char flush with a deadline trigger",
  [
    // > 300 chars, no sentence terminator, contains a 'deadline' keypoint... no,
    // 'deadline' is an ACTION trigger. This whole blob flushes as one sentence.
    "the team spent the entire morning walking through every open thread on the roadmap and the consensus that slowly emerged across all of those conversations was that the single most pressing item we still have to nail down before anything else can move forward is the launch deadline and nobody wanted to commit to one yet",
  ]
);

// Fixture 6: short-sentence skip (<8 chars after trim) and no-trigger transcript
// lines produce zero notes; counters stay at zero.
const skipsAndNoTriggers = runFixture(
  "short sentences and plain transcript produce no notes",
  [
    "Ok. Hi. Yes.",
    "The weather outside is quite pleasant today and nobody mentioned anything actionable.",
  ]
);

// Fixture 7: flush at stop with a trailing unterminated triggering sentence.
const flushTriggerAtStop = runFixture(
  "trailing unterminated decision sentence is caught by flush at stop",
  ["So to wrap up, we chose the Rust rewrite"]
);

// Fixture 8: a line that is itself a question must NOT resolve prior questions
// (OpenQs.consider skips lines ending in '?'), and category precedence
// (decisions before actions before questions before keypoints).
const precedenceAndQuestionSkip = runFixture(
  "category precedence + consider() skips question-terminated lines",
  [
    "Can we get the report by next Friday?",
    "Should we also include the revenue numbers?",
    "We agreed the report is due Friday and assigned to Bob.",
  ]
);

// Fixture 9: STRESS — an adversarial sweep across the regex edge cases that a
// curated one-per-category fixture would miss. Each line probes a specific
// construct that differs subtly between the JS and Rust regex engines if the
// translation is wrong: case-insensitivity (`/i`), `\b` boundaries
// (agreed/todo/never as substrings vs whole words), the weekday `by ...` action
// pattern, `I'll`/`you'll` contraction alternations, the metric `\d+` keypoint,
// the `does anyone`/`anyone` question patterns, the literal-dot wildcard in
// `highest.leverage`, and a question whose keywords carry apostrophes. Driven
// through the SAME live pipeline so counters and open-question resolution are
// exercised end-to-end. This is the byte-identity proof beyond hand-picked rows.
const stressSweep = runFixture(
  "adversarial regex + boundary + case sweep across every category",
  [
    // decisions, mixed case + contraction alt
    "Honestly, We'Ve Decided To rewrite the parser in Rust.",
    // decisions: \bagreed\b must NOT fire on 'disagreed' (boundary) ...
    "Everyone disagreed loudly about the timeline at first.",
    // ... but DOES fire on a standalone 'agreed'.
    "After more talk we finally agreed on the plan.",
    // actions: I'll contraction + verb alternation
    "I'll review the migration script before the demo.",
    // actions: you'll contraction
    "you'll schedule the retro for next week.",
    // actions: weekday 'by ...' pattern (case-insensitive)
    "Please get it done by NEXT thursday at the latest.",
    // actions: \btodo\b boundary — 'todos' should still match (\b after 'todo'?)
    "Add a todo for the logging cleanup.",
    // keypoints: metric \d+%
    "Throughput rose 3x and errors dropped to under 2%.",
    // keypoints: literal-dot wildcard 'highest.leverage'
    "This is the highest-leverage change we can make this quarter.",
    // keypoints: \b(never|always|must|...)\b
    "We must never log raw audio, that is essential.",
    // question: 'does anyone' + trailing '?' (questions[0] wins)
    "Does anyone know if the cache invalidation is correct?",
    // a declarative line that resolves the question (>=2 overlap: cache, invalidation)
    "The cache invalidation logic was fixed and verified yesterday.",
    // plain transcript, no trigger
    "Anyway, lunch is at noon and the room is booked.",
  ]
);

const fixtures = {
  one_per_category: decisionsActionsKeypointsQuestions,
  open_question_resolution: openQuestionResolution,
  multiple_open_questions: multipleOpenQuestions,
  cross_line_buffering: crossLineBuffering,
  long_unpunctuated_flush: longUnpunctuatedFlush,
  skips_and_no_triggers: skipsAndNoTriggers,
  flush_trigger_at_stop: flushTriggerAtStop,
  precedence_and_question_skip: precedenceAndQuestionSkip,
  stress_sweep: stressSweep,
};

mkdirSync(join(HERE, "..", "extractor"), { recursive: true });
for (const [name, fx] of Object.entries(fixtures)) {
  const p = join(HERE, "..", "extractor", `${name}.json`);
  writeFileSync(p, JSON.stringify(fx, null, 1));
  const c = fx.expected.finalCounters;
  console.log(
    "wrote",
    p,
    `— D:${c.decisions} A:${c.actions} K:${c.keypoints} Q-open:${c.questions}`,
    fx.expected.openQuestions.length ? `| open: ${JSON.stringify(fx.expected.openQuestions)}` : ""
  );
}
