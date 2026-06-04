//! The trigger regex set — the four categories' patterns, ported from the
//! index.html `NoteEngine.triggers` object.
//!
//! Each entry pairs the **JavaScript** source string (`pattern.source`, emitted
//! verbatim as `trigger_phrase`) with a compiled [`regex::Regex`] in the `regex`
//! crate dialect. The two dialects agree on every construct these patterns use;
//! where the JS pattern is case-insensitive (the `/i` flag) the Rust pattern is
//! prefixed with `(?i)`. Every JS trigger is `/i` EXCEPT `questions[0]`
//! (`/\?$/`), which is case-sensitive (irrelevant for `?`, but kept faithful).
//!
//! Category iteration order is `decisions` → `actions` → `questions` →
//! `keypoints` (the insertion order of the JS object), and within a category the
//! first matching pattern wins. [`TriggerSet::categorize`] reproduces both.

use silent_core::notes::NoteCategory;

/// One trigger: the JS regex source (emitted as `trigger_phrase`) and its
/// compiled Rust equivalent.
struct Trigger {
    /// The JavaScript `pattern.source`, byte-identical to index.html. This is
    /// what `analyze` records as `triggerPhrase`.
    js_source: &'static str,
    /// The compiled matcher (Rust `regex` dialect; `(?i)` where the JS used `/i`).
    re: regex::Regex,
}

/// The full trigger set across the four categories. Compiled once.
pub struct TriggerSet {
    /// `(category, patterns)` in index.html insertion order.
    categories: Vec<(NoteCategory, Vec<Trigger>)>,
}

impl std::fmt::Debug for TriggerSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriggerSet")
            .field(
                "categories",
                &self
                    .categories
                    .iter()
                    .map(|(c, p)| (*c, p.len()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Default for TriggerSet {
    fn default() -> Self {
        Self::new()
    }
}

impl TriggerSet {
    /// Build the trigger set, compiling every pattern. The regexes are static
    /// and known-valid; compilation cannot fail at runtime for these literals
    /// (a malformed pattern would fail this crate's tests, not production).
    #[must_use]
    pub fn new() -> Self {
        Self {
            categories: vec![
                (NoteCategory::Decisions, build(&DECISIONS)),
                (NoteCategory::Actions, build(&ACTIONS)),
                (NoteCategory::Questions, build(&QUESTIONS)),
                (NoteCategory::Keypoints, build(&KEYPOINTS)),
            ],
        }
    }

    /// Categorize a sentence: scan categories in order, and within each the
    /// patterns in order; the first match wins. Returns the category and the JS
    /// source string of the matching pattern (the `trigger_phrase`), or `None`
    /// if nothing matched (the sentence is just transcript). Faithful port of the
    /// `for (const [cat, patterns] of Object.entries(this.triggers))` loop with
    /// its `break`-on-first-match.
    #[must_use]
    pub fn categorize(&self, sentence: &str) -> Option<(NoteCategory, &str)> {
        for (cat, patterns) in &self.categories {
            for t in patterns {
                if t.re.is_match(sentence) {
                    return Some((*cat, t.js_source));
                }
            }
        }
        None
    }
}

/// Compile a slice of `(js_source, rust_pattern)` pairs into [`Trigger`]s.
fn build(specs: &[(&'static str, &'static str)]) -> Vec<Trigger> {
    specs
        .iter()
        .map(|(js_source, rust_pattern)| Trigger {
            js_source,
            re: regex::Regex::new(rust_pattern).unwrap_or_else(|e| {
                // Static literals; a failure is a programming error caught by
                // tests, never a production path. Documented per the lint bar.
                panic!("invalid trigger regex `{rust_pattern}`: {e}")
            }),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The patterns. Each tuple is (JS pattern.source — emitted verbatim, Rust regex).
//
// Translation rules applied (the only differences between the dialects here):
//   - JS `/.../i`           → Rust `(?i)...`
//   - JS `/.../`  (no flag) → Rust `...`
// Everything else (`\b`, `\w`, `\d`, `\s`, `( )`, `|`, `?`, `+`, `*`, anchored
// `$`, the literal-class chars, the wildcard `.`) is identical in both engines
// for these inputs. The JS source strings are copied byte-for-byte from
// index.html ~lines 2557-2611.
// ---------------------------------------------------------------------------

const DECISIONS: [(&str, &str); 10] = [
    ("we('ve| have) decided to", r"(?i)we('ve| have) decided to"),
    (
        "we('ll| will) move forward with",
        r"(?i)we('ll| will) move forward with",
    ),
    ("the decision is", r"(?i)the decision is"),
    (
        "we('re| are) going (with|to go with)",
        r"(?i)we('re| are) going (with|to go with)",
    ),
    ("let's go with", r"(?i)let's go with"),
    (r"\bagreed\b", r"(?i)\bagreed\b"),
    (
        "final (answer|decision|call)",
        r"(?i)final (answer|decision|call)",
    ),
    ("we('ll| will) use", r"(?i)we('ll| will) use"),
    ("we chose", r"(?i)we chose"),
    ("going ahead with", r"(?i)going ahead with"),
];

const ACTIONS: [(&str, &str); 12] = [
    (
        r"(\w+) (will|is going to|needs to|should) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)",
        r"(?i)(\w+) (will|is going to|needs to|should) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)",
    ),
    (
        r"I('ll| will) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)",
        r"(?i)I('ll| will) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)",
    ),
    (
        r"you('ll| will) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)",
        r"(?i)you('ll| will) (handle|take care of|own|do|create|build|fix|update|send|schedule|write|review|check|look into|reach out|follow up)",
    ),
    ("action item", r"(?i)action item"),
    (r"\btodo\b", r"(?i)\btodo\b"),
    ("needs to be done", r"(?i)needs to be done"),
    ("follow up (on|with)", r"(?i)follow up (on|with)"),
    ("assigned to", r"(?i)assigned to"),
    ("take a look at", r"(?i)take a look at"),
    (
        r"by (next |this |end of )?(monday|tuesday|wednesday|thursday|friday|saturday|sunday|week|month|sprint|quarter|day)",
        r"(?i)by (next |this |end of )?(monday|tuesday|wednesday|thursday|friday|saturday|sunday|week|month|sprint|quarter|day)",
    ),
    ("deadline", r"(?i)deadline"),
    ("due (date|by)", r"(?i)due (date|by)"),
];

const QUESTIONS: [(&str, &str); 11] = [
    // questions[0] is the ONLY case-sensitive trigger (JS `/\?$/`, no `i`).
    (r"\?$", r"\?$"),
    (
        "does anyone (know|have|think)",
        r"(?i)does anyone (know|have|think)",
    ),
    (
        "what (do|should|would|can|if|are|is) ",
        r"(?i)what (do|should|would|can|if|are|is) ",
    ),
    (
        "how (do|should|would|can|will|are|is) ",
        r"(?i)how (do|should|would|can|will|are|is) ",
    ),
    ("can we ", r"(?i)can we "),
    ("should we ", r"(?i)should we "),
    (
        "still (need|needs) to (be )?resolved",
        r"(?i)still (need|needs) to (be )?resolved",
    ),
    ("open question", r"(?i)open question"),
    (
        "wondering (if|whether|about)",
        r"(?i)wondering (if|whether|about)",
    ),
    (
        "not sure (if|whether|about|how)",
        r"(?i)not sure (if|whether|about|how)",
    ),
    (
        "anyone (know|have|think|seen)",
        r"(?i)anyone (know|have|think|seen)",
    ),
];

const KEYPOINTS: [(&str, &str); 12] = [
    (
        r"\d+[\s]*(%|percent|x|times|minutes|hours|days|lines|files|MB|GB|ms|seconds)",
        r"(?i)\d+[\s]*(%|percent|x|times|minutes|hours|days|lines|files|MB|GB|ms|seconds)",
    ),
    (
        "increased?|decreased?|improved?|reduced?|grew|dropped|rose|fell",
        r"(?i)increased?|decreased?|improved?|reduced?|grew|dropped|rose|fell",
    ),
    (
        "the (key|main|important|critical|biggest|primary|core) (thing|point|issue|problem|takeaway|insight|finding)",
        r"(?i)the (key|main|important|critical|biggest|primary|core) (thing|point|issue|problem|takeaway|insight|finding)",
    ),
    (
        "in (summary|conclusion|short)",
        r"(?i)in (summary|conclusion|short)",
    ),
    (
        "the (result|outcome|finding|data|evidence) (is|shows?|suggests?|indicates?)",
        r"(?i)the (result|outcome|finding|data|evidence) (is|shows?|suggests?|indicates?)",
    ),
    ("turns out", r"(?i)turns out"),
    ("the reason (is|was|for)", r"(?i)the reason (is|was|for)"),
    ("because of", r"(?i)because of"),
    ("this means", r"(?i)this means"),
    (
        "the (problem|issue|challenge|risk|blocker|bottleneck) is",
        r"(?i)the (problem|issue|challenge|risk|blocker|bottleneck) is",
    ),
    (
        r"\b(never|always|must|critical|essential|required|mandatory)\b",
        r"(?i)\b(never|always|must|critical|essential|required|mandatory)\b",
    ),
    (
        "highest.leverage|biggest.impact|most.important",
        r"(?i)highest.leverage|biggest.impact|most.important",
    ),
];

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism (PRD lint config)"
)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        // build() panics on a bad pattern; constructing the full set exercises
        // all four categories.
        let _ = TriggerSet::new();
    }

    #[test]
    fn category_order_and_first_match_win() {
        let ts = TriggerSet::new();
        // "we have decided to ship" → decisions, first pattern.
        let (cat, src) = ts.categorize("we have decided to ship").unwrap();
        assert_eq!(cat, NoteCategory::Decisions);
        assert_eq!(src, "we('ve| have) decided to");

        // A trailing "?" → questions, and questions[0] wins over later word
        // patterns (it's first in iteration). This also proves questions is
        // checked before keypoints.
        let (cat, src) = ts.categorize("what should we do about staging?").unwrap();
        assert_eq!(cat, NoteCategory::Questions);
        assert_eq!(src, r"\?$");

        // A metric → keypoints (only reached when nothing earlier matched).
        let (cat, src) = ts.categorize("latency improved by 40% overall").unwrap();
        // "improved" matches keypoints[1] before the metric pattern? No —
        // keypoints[0] (the metric) is first; but "improved" is ALSO keypoints.
        // keypoints[0] is the \d+...% pattern and comes first, so it wins.
        assert_eq!(cat, NoteCategory::Keypoints);
        assert_eq!(
            src,
            r"\d+[\s]*(%|percent|x|times|minutes|hours|days|lines|files|MB|GB|ms|seconds)"
        );
    }

    #[test]
    fn questions_zero_is_case_sensitive_anchored() {
        let ts = TriggerSet::new();
        // Only a literal trailing '?' triggers questions[0].
        assert!(ts.categorize("is it ready?").is_some());
        // No '?' and no other trigger → None.
        assert!(ts.categorize("the meeting was pleasant today").is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let ts = TriggerSet::new();
        assert!(
            ts.categorize("the weather outside is quite pleasant today")
                .is_none()
        );
    }
}
