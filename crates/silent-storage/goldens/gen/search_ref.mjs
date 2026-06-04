// Reference meeting-history search — a faithful, DOM-free port of the JS in
// index.html (openHistory ~5103, _filterHistory ~5164). This is the BEHAVIOR
// CONTRACT the Rust port (silent-storage `search` module) must reproduce.
//
// Run: node search_ref.mjs  → writes ../search/*.json
//
// CURRENT JS behavior — captured verbatim, NOT "improved":
//
//   openHistory():
//     db.meetings.orderBy('startTime').reverse().limit(50).toArray()
//       → the candidate set is the most-recent 50 meetings, newest first.
//
//   _filterHistory(allMeetings, rawQuery):
//     const q = rawQuery.trim().toLowerCase();
//     if (!q) return allMeetings;                 // empty query → all (in order)
//     for (const m of allMeetings) {              // preserves the newest-first order
//       if ((m.title||'').toLowerCase().includes(q)) { match; continue; }
//       if (anyNote.text.toLowerCase().includes(q)) { match; continue; }
//       if (anyChunk.text.toLowerCase().includes(q)) { match; continue; }
//     }
//
// So "fuzzy" here is a case-insensitive SUBSTRING match across title, then notes,
// then transcript chunks; result order is the input order (startTime desc). The
// Rust port is a pure function: (meetings, notes, chunks, query, limit) → ranked
// ids. "Ranked" == newest-first, capped at `limit` (50). We pin the id order.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "..", "search");
mkdirSync(OUT, { recursive: true });

// ── exact JS pipeline over typed records ─────────────────────────────────────
// meetings: [{id, title, startTime}]; notes: [{meetingId, text}];
// chunks: [{meetingId, text}].
function lastN(meetings, n) {
  // orderBy('startTime').reverse().limit(n): newest first, capped.
  return [...meetings]
    .sort((a, b) => a.startTime - b.startTime)
    .reverse()
    .slice(0, n);
}

function filter(meetings, notes, chunks, rawQuery) {
  const q = String(rawQuery).trim().toLowerCase();
  if (!q) return meetings.map((m) => m.id);
  const out = [];
  for (const m of meetings) {
    if ((m.title || "").toLowerCase().includes(q)) {
      out.push(m.id);
      continue;
    }
    const notesHit = notes.some(
      (n) => n.meetingId === m.id && (n.text || "").toLowerCase().includes(q),
    );
    if (notesHit) {
      out.push(m.id);
      continue;
    }
    const chunkHit = chunks.some(
      (c) => c.meetingId === m.id && (c.text || "").toLowerCase().includes(q),
    );
    if (chunkHit) {
      out.push(m.id);
      continue;
    }
  }
  return out;
}

function run(meetings, notes, chunks, query, limit) {
  const candidates = lastN(meetings, limit);
  return filter(candidates, notes, chunks, query);
}

function write(name, obj) {
  writeFileSync(join(OUT, name), JSON.stringify(obj, null, 1) + "\n");
}

// ── fixtures ─────────────────────────────────────────────────────────────────
const meetings = [
  { id: 1, title: "Kickoff", startTime: 1000 },
  { id: 2, title: "Design Review", startTime: 3000 },
  { id: 3, title: "Budget Sync", startTime: 2000 },
  { id: 4, title: "RETRO (Q2)", startTime: 5000 }, // uppercase → case-insensitive
  { id: 5, title: "", startTime: 4000 }, // empty title (Untitled)
];
const notes = [
  { meetingId: 1, text: "Decided to use Rust" },
  { meetingId: 3, text: "Privacy is the wedge" },
  { meetingId: 5, text: "Action: ship the registry" },
];
const chunks = [
  { meetingId: 2, text: "let us talk about the diarization pipeline" },
  { meetingId: 4, text: "the retrospective covered velocity" },
  { meetingId: 5, text: "we should pin every model revision" },
];

write("order_newest_first.json", {
  description:
    "empty query → all candidates newest-first by startTime (ids 4,5,2,3,1)",
  input: { meetings, notes, chunks, query: "", limit: 50 },
  expected: run(meetings, notes, chunks, "", 50),
});

write("title_match.json", {
  description: "case-insensitive substring match on title ('review' → Design Review)",
  input: { meetings, notes, chunks, query: "review", limit: 50 },
  expected: run(meetings, notes, chunks, "review", 50),
});

write("title_case_insensitive.json", {
  description: "lowercased query matches uppercase title ('retro' → RETRO (Q2))",
  input: { meetings, notes, chunks, query: "retro", limit: 50 },
  expected: run(meetings, notes, chunks, "retro", 50),
});

write("notes_match.json", {
  description: "no title hit; matches via a note's text ('wedge' → Budget Sync id 3)",
  input: { meetings, notes, chunks, query: "wedge", limit: 50 },
  expected: run(meetings, notes, chunks, "wedge", 50),
});

write("transcript_match.json", {
  description:
    "no title/notes hit; matches via a transcript chunk ('diarization' → Design Review id 2)",
  input: { meetings, notes, chunks, query: "diarization", limit: 50 },
  expected: run(meetings, notes, chunks, "diarization", 50),
});

write("multi_field_order.json", {
  description:
    "'pin' hits only chunk of id 5; 'ship' hits title none/notes id5 — combined query 'i' matches many, order preserved newest-first",
  input: { meetings, notes, chunks, query: "i", limit: 50 },
  expected: run(meetings, notes, chunks, "i", 50),
});

write("no_match.json", {
  description: "query matching nothing → empty result",
  input: { meetings, notes, chunks, query: "zzzznotfound", limit: 50 },
  expected: run(meetings, notes, chunks, "zzzznotfound", 50),
});

write("whitespace_query_is_empty.json", {
  description: "whitespace-only query trims to empty → all candidates",
  input: { meetings, notes, chunks, query: "   ", limit: 50 },
  expected: run(meetings, notes, chunks, "   ", 50),
});

{
  // limit cap: 60 meetings, only newest 50 are candidates. ids 1..60 with
  // startTime == id*1000, so newest-first is 60,59,...,11 (50 of them); id 5
  // (with a matching note) is NOT a candidate because it's older than the top 50.
  const many = [];
  for (let i = 1; i <= 60; i++) many.push({ id: i, title: `M${i}`, startTime: i * 1000 });
  const manyNotes = [{ meetingId: 5, text: "needle" }, { meetingId: 55, text: "needle" }];
  write("limit_50_cap.json", {
    description:
      "candidate set is newest 50; a matching meeting outside the top-50 window is excluded (id 5 dropped, id 55 kept)",
    input: { meetings: many, notes: manyNotes, chunks: [], query: "needle", limit: 50 },
    expected: run(many, manyNotes, [], "needle", 50),
  });
}

console.log("wrote search goldens to", OUT);
