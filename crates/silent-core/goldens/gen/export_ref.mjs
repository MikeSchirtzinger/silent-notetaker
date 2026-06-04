// Reference exporters — a faithful, DOM-free port of the export/copy formatting
// in index.html. This is the BEHAVIOR CONTRACT the Rust port (silent-core
// `export` module) must reproduce exactly.
//
// Run: node export_ref.mjs  → writes ../export/*.json
//
// The shipping functions read their inputs from the DOM (meeting title input,
// `.note-item`/`.note-text`/`.note-time` text, `.transcript-item` text) and
// format wall-clock dates via Intl. The Rust core is DOM-/Intl-free: it takes
// the SAME values as typed records and produces the SAME strings. This harness
// drives the EXACT JS string-assembly logic over typed fixtures so the markdown
// is pinned byte-for-byte. Date/duration strings are passed in as already-
// formatted values (the orchestrator computes them) — what's under test here is
// the markdown structure, the per-item prefix/filter, and the summary executive
// line, none of which touch the DOM or Intl.
//
// JS captured:
//
//   notesToMarkdown():    `# ${title}\n**Date:** ${date}  **Duration:** ${dur}\n\n`
//     per section (decisions/actions/keypoints/questions, in that order):
//       items = list.map(item => withTime && t ? `- [${t}] ${text}` : `- ${text}`)
//              .filter(line => line.replace(/^- (\[[^\]]*\] )?/, '').length > 0)
//       if items.length: md += `${header}\n${items.join('\n')}\n\n`
//     return md.trim();
//
//   openMeetingDetail (history replay export): same shape but ALWAYS `- ${text}`
//     (no per-line timestamps), category order decisions→actions→keypoints→questions,
//     skipping empty sections.
//
//   generateSummary().executiveLine:
//     parts=[]; if(decisions) `${n} decision${n>1?'s':''} made`;
//               if(actions)   `${n} action item${n>1?'s':''} assigned`;
//               if(questions) `${n} open question${n>1?'s':''}`;
//     parts.length ? `${dur} meeting with ${parts.join(', ')}.`
//                  : `${dur} meeting recorded. ${totalWords} words transcribed.`
//
//   copySummaryMarkdown(): notesToMarkdown() then, if AI notes exist, append
//     `\n\n## AI Meeting Notes (on-device · Qwen)\n` and per group
//     `\n### ${label}\n` + items, each item:
//       chip ? `- **${chip}** — ${text}` : `- ${text}`   (then md.trim()).
//
//   copyTranscript(): per transcript line withTime && t ? `[${t}] ${txt}` : txt,
//     filter(Boolean), join('\n').

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "..", "export");
mkdirSync(OUT, { recursive: true });

const SECTION_ORDER = ["decisions", "actions", "keypoints", "questions"];
const NOTES_HEADERS = {
  decisions: "## Decisions",
  actions: "## Action Items",
  keypoints: "## Key Points",
  questions: "## Open Questions",
};
const DETAIL_HEADERS = {
  decisions: "## Decisions",
  actions: "## Action Items",
  keypoints: "## Key Points",
  questions: "## Open Questions",
};

// ── notesToMarkdown: notes carry {category, text, time?} ─────────────────────
// `withTime` mirrors loadSettings().showTimestamps !== false. `time` is the
// already-formatted per-line stamp string (what `.note-time` textContent holds).
function notesToMarkdown(title, date, duration, notes, withTime) {
  const t = (title || "").trim() || "Meeting Notes";
  let md = `# ${t}\n**Date:** ${date}  **Duration:** ${duration}\n\n`;
  for (const cat of SECTION_ORDER) {
    const items = notes
      .filter((n) => n.category === cat)
      .map((n) => {
        const text = (n.text || "").trim();
        const ts = (n.time || "").trim();
        return withTime && ts ? `- [${ts}] ${text}` : `- ${text}`;
      })
      .filter((line) => line.replace(/^- (\[[^\]]*\] )?/, "").length > 0);
    if (items.length > 0) {
      md += `${NOTES_HEADERS[cat]}\n${items.join("\n")}\n\n`;
    }
  }
  return md.trim();
}

// ── history replay export (openMeetingDetail) ────────────────────────────────
function historyReplayMarkdown(title, date, duration, notes) {
  const t = (title || "").trim() || "Meeting Notes";
  let md = `# ${t}\n**Date:** ${date}  **Duration:** ${duration}\n\n`;
  for (const cat of SECTION_ORDER) {
    const items = notes
      .filter((n) => n.category === cat)
      .map((n) => n.text);
    if (items.length > 0) {
      md += `${DETAIL_HEADERS[cat]}\n${items.map((x) => `- ${x}`).join("\n")}\n\n`;
    }
  }
  return md.trim();
}

// ── summary executive line ───────────────────────────────────────────────────
function executiveLine(duration, notes, totalWords) {
  const count = (cat) => notes.filter((n) => n.category === cat).length;
  const d = count("decisions");
  const a = count("actions");
  const q = count("questions");
  const parts = [];
  if (d) parts.push(`${d} decision${d > 1 ? "s" : ""} made`);
  if (a) parts.push(`${a} action item${a > 1 ? "s" : ""} assigned`);
  if (q) parts.push(`${q} open question${q > 1 ? "s" : ""}`);
  return parts.length > 0
    ? `${duration} meeting with ${parts.join(", ")}.`
    : `${duration} meeting recorded. ${totalWords} words transcribed.`;
}

// ── copyTranscript ───────────────────────────────────────────────────────────
// lines carry {time, text}; withTime mirrors showTimestamps !== false.
function transcriptText(lines, withTime) {
  return lines
    .map((l) => {
      const t = (l.time || "").trim();
      const txt = (l.text || "").trim();
      return withTime && t ? `[${t}] ${txt}` : txt;
    })
    .filter(Boolean)
    .join("\n");
}

// ── copySummaryMarkdown AI-notes append ──────────────────────────────────────
// aiGroups: [{label, items:[{chip?, text}]}]. Appended to a base notes markdown.
function summaryMarkdownWithAi(baseMd, aiGroups) {
  let md = baseMd;
  if (aiGroups && aiGroups.length) {
    md += "\n\n## AI Meeting Notes (on-device · Qwen)\n";
    for (const g of aiGroups) {
      const items = g.items.map((i) =>
        i.chip ? `- **${i.chip}** — ${i.text}` : `- ${i.text}`,
      );
      if (items.length) md += `\n### ${g.label}\n${items.join("\n")}\n`;
    }
    md = md.trim();
  }
  return md;
}

function write(name, obj) {
  writeFileSync(join(OUT, name), JSON.stringify(obj, null, 1) + "\n");
}

// ── fixtures ─────────────────────────────────────────────────────────────────
const fullNotes = [
  { category: "decisions", text: "Ship v2 in June", time: "00:12" },
  { category: "decisions", text: "Drop the legacy importer", time: "01:40" },
  { category: "actions", text: "Alice writes the migration", time: "02:05" },
  { category: "keypoints", text: "Users cited the privacy wedge", time: "03:21" },
  { category: "keypoints", text: "  ", time: "03:30" }, // whitespace-only → dropped
  { category: "questions", text: "Who owns the rollout?", time: "04:10" },
];

write("notes_with_time.json", {
  description:
    "notesToMarkdown with timestamps on; whitespace-only note dropped by the filter; section order decisions→actions→keypoints→questions",
  input: {
    title: "Q3 Planning",
    date: "June 4, 2026",
    duration: "05:00",
    withTime: true,
    notes: fullNotes,
  },
  expected: notesToMarkdown(
    "Q3 Planning",
    "June 4, 2026",
    "05:00",
    fullNotes,
    true,
  ),
});

write("notes_no_time.json", {
  description: "notesToMarkdown with timestamps off → `- text`, no `[ts]` prefix",
  input: {
    title: "  Q3 Planning  ",
    date: "June 4, 2026",
    duration: "05:00",
    withTime: false,
    notes: fullNotes,
  },
  expected: notesToMarkdown(
    "  Q3 Planning  ",
    "June 4, 2026",
    "05:00",
    fullNotes,
    false,
  ),
});

write("notes_empty_title.json", {
  description:
    "empty/whitespace title falls back to 'Meeting Notes'; no notes → header only after trim",
  input: {
    title: "   ",
    date: "June 4, 2026",
    duration: "00:00",
    withTime: true,
    notes: [],
  },
  expected: notesToMarkdown("   ", "June 4, 2026", "00:00", [], true),
});

write("history_replay.json", {
  description:
    "history replay export (openMeetingDetail): always `- text`, no per-line stamps; empty sections skipped",
  input: {
    title: "Past Standup",
    date: "6/3/2026, 9:00:00 AM",
    duration: "12m 3s",
    notes: [
      { category: "decisions", text: "Use Turso for local storage" },
      { category: "actions", text: "Bob benchmarks IndexedDB" },
      { category: "questions", text: "Do we need a service worker?" },
    ],
  },
  expected: historyReplayMarkdown(
    "Past Standup",
    "6/3/2026, 9:00:00 AM",
    "12m 3s",
    [
      { category: "decisions", text: "Use Turso for local storage" },
      { category: "actions", text: "Bob benchmarks IndexedDB" },
      { category: "questions", text: "Do we need a service worker?" },
    ],
  ),
});

write("executive_line_full.json", {
  description: "executive line with decisions+actions+questions (pluralized)",
  input: { duration: "05:00", notes: fullNotes, totalWords: 1234 },
  expected: executiveLine("05:00", fullNotes, 1234),
});

write("executive_line_singular.json", {
  description: "executive line with exactly one of each (singular forms)",
  input: {
    duration: "01:00",
    notes: [
      { category: "decisions", text: "x" },
      { category: "actions", text: "y" },
      { category: "questions", text: "z" },
    ],
    totalWords: 50,
  },
  expected: executiveLine(
    "01:00",
    [
      { category: "decisions", text: "x" },
      { category: "actions", text: "y" },
      { category: "questions", text: "z" },
    ],
    50,
  ),
});

write("executive_line_empty.json", {
  description: "executive line with no decisions/actions/questions → words fallback",
  input: {
    duration: "00:30",
    notes: [{ category: "keypoints", text: "only a key point" }],
    totalWords: 9,
  },
  expected: executiveLine(
    "00:30",
    [{ category: "keypoints", text: "only a key point" }],
    9,
  ),
});

write("transcript_with_time.json", {
  description: "copyTranscript with timestamps on → `[ts] text`; empty lines dropped",
  input: {
    withTime: true,
    lines: [
      { time: "00:00", text: "Hello everyone." },
      { time: "00:03", text: "  Let's begin.  " },
      { time: "00:05", text: "   " }, // empty after trim → dropped
      { time: "", text: "No stamp line" }, // no time → bare text even with withTime
    ],
  },
  expected: transcriptText(
    [
      { time: "00:00", text: "Hello everyone." },
      { time: "00:03", text: "  Let's begin.  " },
      { time: "00:05", text: "   " },
      { time: "", text: "No stamp line" },
    ],
    true,
  ),
});

write("transcript_no_time.json", {
  description: "copyTranscript with timestamps off → bare text lines",
  input: {
    withTime: false,
    lines: [
      { time: "00:00", text: "Hello everyone." },
      { time: "00:03", text: "Let's begin." },
    ],
  },
  expected: transcriptText(
    [
      { time: "00:00", text: "Hello everyone." },
      { time: "00:03", text: "Let's begin." },
    ],
    false,
  ),
});

{
  const base = notesToMarkdown(
    "Q3 Planning",
    "June 4, 2026",
    "05:00",
    fullNotes,
    true,
  );
  const aiGroups = [
    {
      label: "Discussion",
      items: [
        { chip: "Decision", text: "Ship v2 in June" },
        { chip: "Key point", text: "Privacy is the wedge" },
      ],
    },
    {
      label: "Action Items",
      items: [{ text: "Alice writes the migration" }],
    },
  ];
  write("summary_with_ai_notes.json", {
    description:
      "copySummaryMarkdown: notes markdown + appended AI Meeting Notes groups (chip → `- **chip** — text`)",
    input: {
      title: "Q3 Planning",
      date: "June 4, 2026",
      duration: "05:00",
      withTime: true,
      notes: fullNotes,
      aiGroups,
    },
    expected: summaryMarkdownWithAi(base, aiGroups),
  });
}

console.log("wrote export goldens to", OUT);
