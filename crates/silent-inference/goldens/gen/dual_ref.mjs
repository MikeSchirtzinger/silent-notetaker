// Reference Dual-mode draft/refine coordination — a faithful, DOM-free port of
// the index.html dual-mode interleaving that crates/silent-inference ports to
// Rust. This file is the BEHAVIOR CONTRACT (Appendix A row 11): the Rust
// `DualCoordinator` must reproduce these transcript-list states event-for-event.
//
// Sourced verbatim from index.html:
//   - startDualModel()   ~line 2970 — Moonshine worker 'final' → onPartial (draft);
//                         SenseVoice onResult → onFinal (refined)
//   - handlePartial(text) ~line 4269 — dual branch:
//       renderTranscriptItem(timestamp, text.trim(), true)   // true = draft
//   - handleFinal(text)   ~line 4285 — dual branch:
//       const drafts = container.querySelectorAll('.transcript-draft');
//       const toRemove = [...drafts].slice(0, Math.max(0, drafts.length - 1));
//       toRemove.forEach(d => d.remove());
//       // then the refined (non-draft) item is rendered
//
// The DOM is modeled as an ordered list of transcript items, each
// `{ text, draft: bool }`. `renderTranscriptItem(ts, text, draft)` appends one
// item. `querySelectorAll('.transcript-draft')` is "every item with draft===true,
// in document order". `.remove()` deletes that item from the list.
//
// What this pins (the policy, all Rust-portable, no DOM, no audio, no model):
//   1. Moonshine final → append a DRAFT item (italic/dim in the UI).
//   2. SenseVoice final → BEFORE appending the refined item, remove all but the
//      LAST draft currently in the list (`slice(0, drafts.length - 1)`), i.e.
//      keep at most one draft as a "preview" of upcoming content; then append the
//      refined NON-draft item.
//   3. Empty/whitespace text is ignored (handlePartial: `if (!text.trim()) return`;
//      handleFinal: `if (!text.trim() || !meetingId) return`). meetingId is
//      assumed present (recording active) in these fixtures.
//
// Run: node dual_ref.mjs  → writes ../dual/interleavings.json
//
// PURE FUNCTION of the event stream — exactly reproducible. The Rust golden test
// (tests/dual_golden.rs) replays each event stream and asserts the list states
// match byte-for-byte.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// =====================================================================
// Exact JS port of the dual-mode coordinator over a list model.
// =====================================================================

class DualModel {
  constructor() {
    // Ordered transcript items: { text, draft }.
    this.items = [];
  }

  // index.html renderTranscriptItem(timestamp, text, isDraft, speakerInfo) —
  // appends one item. We drop timestamp/speakerInfo (not part of the interleaving
  // policy; the Rust port keeps text + draft flag only).
  _render(text, draft) {
    this.items.push({ text, draft });
  }

  // handlePartial(text) — dual branch.
  moonshineFinal(text) {
    // `if (!text.trim()) return;`
    if (text.trim().length === 0) return;
    // dual: render as DRAFT, text.trim().
    this._render(text.trim(), true);
  }

  // handleFinal(text) — dual branch (meetingId assumed present).
  senseVoiceFinal(text) {
    // `if (!text.trim() || !this.meetingId) return;`
    if (text.trim().length === 0) return;
    // Remove recent draft items that SenseVoice is replacing; keep at most 1 as
    // preview: `toRemove = drafts.slice(0, Math.max(0, drafts.length - 1))`.
    const draftIdxs = this.items
      .map((it, i) => (it.draft ? i : -1))
      .filter((i) => i >= 0);
    const removeCount = Math.max(0, draftIdxs.length - 1);
    const removeSet = new Set(draftIdxs.slice(0, removeCount));
    this.items = this.items.filter((_, i) => !removeSet.has(i));
    // Then render the refined NON-draft item (text.trim()).
    this._render(text.trim(), false);
  }

  snapshot() {
    return this.items.map((it) => ({ text: it.text, draft: it.draft }));
  }
}

// Apply one event stream and capture the list state AFTER EACH event, so the
// Rust port is checked step-by-step (catching any divergence at the exact event).
function runStream(events) {
  const m = new DualModel();
  const steps = [];
  for (const ev of events) {
    if (ev.kind === "moonshine") m.moonshineFinal(ev.text);
    else if (ev.kind === "sensevoice") m.senseVoiceFinal(ev.text);
    else throw new Error("unknown event kind: " + ev.kind);
    steps.push({ after: ev, items: m.snapshot() });
  }
  return { events, steps, final: m.snapshot() };
}

// =====================================================================
// Fixture event streams. Each exercises a facet of the draft/refine rule.
// =====================================================================

const cases = [
  {
    name: "single_draft_then_refine",
    // One Moonshine draft, then SenseVoice refines: with exactly 1 draft,
    // removeCount = max(0, 1-1) = 0 → the draft stays, refined appended AFTER it.
    events: [
      { kind: "moonshine", text: "helo wrld" },
      { kind: "sensevoice", text: "hello world" },
    ],
  },
  {
    name: "multiple_drafts_then_refine_keeps_last_preview",
    // Three drafts accumulate (Moonshine is faster), then SenseVoice fires:
    // removeCount = max(0, 3-1) = 2 → the first two drafts are removed, the LAST
    // draft is kept as a preview, refined item appended after it.
    events: [
      { kind: "moonshine", text: "draft one" },
      { kind: "moonshine", text: "draft two" },
      { kind: "moonshine", text: "draft three" },
      { kind: "sensevoice", text: "refined first segment" },
    ],
  },
  {
    name: "interleaved_steady_state",
    // Realistic interleave: drafts and refines alternate. After the first refine a
    // single preview draft remains; subsequent drafts add to it; each refine trims
    // back to one preview.
    events: [
      { kind: "moonshine", text: "the quick" },
      { kind: "moonshine", text: "brown fox" },
      { kind: "sensevoice", text: "The quick brown fox" },
      { kind: "moonshine", text: "jumps over" },
      { kind: "moonshine", text: "the lazy" },
      { kind: "moonshine", text: "dog today" },
      { kind: "sensevoice", text: "jumps over the lazy dog." },
    ],
  },
  {
    name: "refine_with_no_drafts",
    // SenseVoice fires before any Moonshine draft (SenseVoice can be first if
    // Moonshine is still warming): removeCount = max(0, 0-1) = 0, nothing removed,
    // refined item appended to an empty list.
    events: [
      { kind: "sensevoice", text: "no drafts yet here" },
      { kind: "moonshine", text: "late draft" },
      { kind: "sensevoice", text: "second refined line" },
    ],
  },
  {
    name: "empty_and_whitespace_ignored",
    // Empty / whitespace events are no-ops (the `!text.trim()` guards).
    events: [
      { kind: "moonshine", text: "" },
      { kind: "moonshine", text: "   " },
      { kind: "sensevoice", text: "\t\n " },
      { kind: "moonshine", text: "  real draft  " }, // trims to "real draft"
      { kind: "sensevoice", text: "  real final  " }, // trims to "real final"
    ],
  },
  {
    name: "back_to_back_refines",
    // Two SenseVoice finals with no draft between the second: the second refine
    // sees only non-draft items (drafts.length 0) → removes nothing, appends.
    events: [
      { kind: "moonshine", text: "d1" },
      { kind: "sensevoice", text: "final one" },
      { kind: "sensevoice", text: "final two" },
    ],
  },
];

const fixture = {
  description:
    "DOM-free goldens for index.html dual-mode draft/refine interleaving (Appendix A row 11)",
  cases: cases.map((c) => ({ name: c.name, ...runStream(c.events) })),
};

mkdirSync(join(HERE, "..", "dual"), { recursive: true });
const p = join(HERE, "..", "dual", "interleavings.json");
writeFileSync(p, JSON.stringify(fixture, null, 1) + "\n");
console.log("wrote", p, "—", fixture.cases.length, "interleaving cases");
