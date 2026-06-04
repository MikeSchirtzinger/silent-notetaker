// Reference word-corrections policy — a faithful, DOM-free port of the JS
// word-correction application in index.html. This is the BEHAVIOR CONTRACT the
// Rust port (crates/silent-notes/src/corrections.rs) must reproduce
// BYTE-IDENTICALLY.
//
// Run: node corrections_ref.mjs  → writes ../corrections/*.json
//
// What is ported, verbatim from index.html (on rust-refactor):
//   - the worker live-application function `applyCorrections(text)` (~line 1671):
//       for (const [wrong, right] of Object.entries(config.corrections || {})) {
//         const re = new RegExp(wrong.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
//         text = text.replace(re, right);
//       }
//       return text;
//   - the main-thread retroactive re-application `applyCorrectionsToTranscript`
//     (~line 5883) uses the IDENTICAL escape + `new RegExp(..., 'gi')` +
//     `String.replace` policy (it just targets DOM nodes instead of one string).
//   - the corrections store is the plain `{ "wrong": "right" }` object
//     (index.html `let corrections = {}`, `corrections[wrong] = right`,
//     `delete corrections[wrong]`). Object iteration order = string-key
//     insertion order, so corrections apply in the order they were ADDED.
//
// The escape set and the `gi` flags are copied character-for-character so the
// Rust port (regex::escape + a case-insensitive, replace-all matcher applied in
// insertion order) is byte-identical. Do NOT "tidy" them.
//
// DOM coupling is removed faithfully: applyCorrectionsToTranscript reads/writes
// `textContent`; the policy applied to that text is exactly `applyCorrections`,
// so a single string-in/string-out reference captures both call sites.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// =====================================================================
// applyCorrections — port of the index.html policy (~lines 1671-1677).
// `corrections` is an array of [wrong, right] pairs preserving INSERTION
// ORDER (the JS object's Object.entries order). Each pair is applied in turn,
// so later corrections see the output of earlier ones — exactly as the JS
// sequential `text = text.replace(...)` loop does.
//
// ONE DELIBERATE PARITY CHOICE: the index.html `text.replace(re, right)` passes
// `right` to String.prototype.replace, which interprets `$&`/`$n`/`$$`/etc. in
// the REPLACEMENT string specially. The Rust port replaces LITERALLY
// (`regex::NoExpand`), so a `$`-containing replacement is inserted verbatim.
// The corrections UX fixes mis-heard *words* (the replacement is a plain word),
// so this only diverges for replacements containing `$`-escape sequences — an
// input the feature never produces meaningfully. To keep golden == Rust
// byte-identical we model that literal-replacement semantics HERE too (replace
// the match span with `right` verbatim, no `$` expansion), and we deliberately
// avoid any `$`-in-replacement fixture so this reference matches the shipping
// JS for every realistic input.
// =====================================================================
function applyCorrections(text, corrections) {
  for (const [wrong, right] of corrections) {
    // The JS escape set: . * + ? ^ $ { } ( ) | [ ] \  (NOT - or /).
    const re = new RegExp(wrong.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
    // Literal replacement (a function callback bypasses String.replace's `$`
    // substitution), matching the Rust port's `NoExpand`.
    text = text.replace(re, () => right);
  }
  return text;
}

// =====================================================================
// Fixture builder — runs each input line through applyCorrections with the
// given ordered correction pairs and records the output, so the Rust golden
// test can assert byte-identical results line-by-line.
// =====================================================================
function runFixture(description, corrections, inputs) {
  const cases = inputs.map((input) => ({
    input,
    output: applyCorrections(input, corrections),
  }));
  return { description, corrections, cases };
}

// =====================================================================
// Fixtures — cover every distinct subtlety of the JS policy.
// =====================================================================

// Fixture 1: basic case-insensitive, global replacement (the common case —
// the ASR mis-hears a proper noun).
const basic = runFixture(
  "basic case-insensitive global replacement of a mis-heard name",
  [["kuber netes", "Kubernetes"]],
  [
    "We deployed it to kuber netes yesterday.",
    "Kuber Netes scales well, and kuber netes is stable.",
    "No match here at all.",
  ]
);

// Fixture 2: the `gi` flag — ALL occurrences in a line, any case, are replaced.
const globalAllCase = runFixture(
  "global flag replaces every occurrence; insensitive flag ignores case",
  [["jira", "Linear"]],
  ["jira JIRA Jira jIrA tracks the work in jira."]
);

// Fixture 3: regex-special characters in the WRONG word must be treated as
// LITERALS (the escape). `c++` must match the literal "c++", not "c" + one-or-
// more "+". A `.` in the wrong word matches a literal dot, not any char.
const regexSpecialEscaped = runFixture(
  "regex metachars in the wrong word are escaped to literals",
  [
    ["c++", "C++"],
    ["node.js", "Node.js"],
    ["a(b)", "A-B"],
    ["$var", "VAR"],
    ["a|b", "A_OR_B"],
    ["x[1]", "X1"],
  ],
  [
    "I wrote it in c++ not in cxx.",
    "node.js and nodexjs differ (the dot is literal).",
    "the token a(b) appears once; aXb does not match.",
    "the price is $var dollars.",
    "choose a|b from the menu.",
    "index x[1] then x.1 stays.",
  ]
);

// Fixture 4: multiple corrections apply in INSERTION ORDER, and a later
// correction can act on an earlier correction's output (chaining).
const insertionOrderChaining = runFixture(
  "corrections apply in insertion order; later ones see earlier output",
  [
    ["foo", "bar"],
    ["bar", "baz"],
  ],
  [
    // "foo" -> "bar" -> "baz"; a pre-existing "bar" also -> "baz".
    "foo and bar both end up the same.",
  ]
);

// Fixture 5: substring replacement (no word-boundary guard in the JS) — the
// wrong word matches INSIDE larger words too, faithfully reproducing the JS
// behavior (which uses no \b). This is an imperfection preserved for parity.
const substringNoBoundary = runFixture(
  "no word-boundary guard: the pattern matches inside larger words too",
  [["cat", "dog"]],
  ["the cat sat; concatenate the category."]
);

// Fixture 6: ordinary multi-pair replacement with plain-word replacements (the
// realistic feature: fixing several mis-heard words). No `$` in any replacement
// (see the parity note on applyCorrections); the literal replacement here is
// identical to the shipping JS for these inputs.
const multiPair = runFixture(
  "several plain-word corrections applied to one line",
  [
    ["teh", "the"],
    ["wrng", "right"],
  ],
  ["teh quick brown fox; wrng becomes right."]
);

// Fixture 7: unicode in both the wrong and right words (BMP) — case-insensitive
// matching over accented text.
const unicode = runFixture(
  "unicode (BMP) wrong/right words, case-insensitive",
  [
    ["cafe", "café"],
    ["MÜNCHEN", "Munich"],
  ],
  ["meet me at the cafe in münchen, the Cafe is nice."]
);

// Fixture 8: empty input, whitespace-only input, and a no-op correction map
// produce the input unchanged.
const edgeEmpty = runFixture(
  "empty / no-match inputs are returned unchanged",
  [["zzz", "qqq"]],
  ["", "   ", "nothing to fix here"]
);

const fixtures = {
  basic,
  global_all_case: globalAllCase,
  regex_special_escaped: regexSpecialEscaped,
  insertion_order_chaining: insertionOrderChaining,
  substring_no_boundary: substringNoBoundary,
  multi_pair: multiPair,
  unicode,
  edge_empty: edgeEmpty,
};

mkdirSync(join(HERE, "..", "corrections"), { recursive: true });
for (const [name, fx] of Object.entries(fixtures)) {
  const p = join(HERE, "..", "corrections", `${name}.json`);
  writeFileSync(p, JSON.stringify(fx, null, 1));
  console.log("wrote", p, `— ${fx.cases.length} cases, ${fx.corrections.length} corrections`);
}
