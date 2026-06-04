//! Wasm-bindgen notes + smart-questions + Qwen surface (PRD Phase 3, Tasks
//! G1/G2; Appendix A rows 16, 18, 19, 21, 22, 35-partial).
//!
//! Exposes the `silent-notes` policy to the browser UI — the same strangler-fig
//! pattern as [`crate::diarization`] wraps `silent-diarization`. The JS glue
//! (`notes-engine.js`) loads the wasm-pack output (`pkg/`) and drives these
//! methods; they return serde-JSON values matching the typed boundary shapes in
//! `silent-core/src/notes.rs` and `silent-core/src/questions.rs`.
//!
//! # What this wraps
//!
//! - [`WasmNoteEngine`] — [`silent_notes::NoteExtractor`] (the live trigger
//!   policy) + [`silent_notes::OpenQuestions`] (open-question tracking). The
//!   live-pipeline call order is preserved by the glue, not the wasm object:
//!   per final line `analyze()` → `consider()` BEFORE the add loop, then the JS
//!   assigns the db id and calls `add(id, text)` for question notes; at stop,
//!   `flush()` (flushed questions are NOT added to the open-question tracker).
//! - [`WasmQuestionScheduler`] — [`silent_notes::questions::Scheduler`], the
//!   `SmartQ` teleprompter scheduler. The Qwen worker (`question-worker.js`)
//!   stays the executor: the scheduler emits `GenerateRequest`s the glue
//!   forwards, and the worker reply is routed back via `on_worker_result`.
//! - The Qwen final-notes free functions ([`parse_qwen_notes`],
//!   [`chunk_transcript`], [`dedupe_notes`], [`final_notes_chunks`]) and the
//!   stop-time recap line cleaner ([`recap_clean_group`]).
//!
//! # ids are bigint at the boundary
//!
//! ts-rs emits `NoteEvent`/`note_added.id` and `question_resolved.id` as
//! `bigint` (Rust `u64`). The note ids the UI uses for `db.notes` are JS
//! numbers; the glue coerces between the two (`Number(id)` / `BigInt(id)`),
//! exactly as the g1 wiring note prescribes. This surface accepts/returns `f64`
//! for the ids it carries so the JS `db.notes.add` number flows through
//! unchanged (a Dexie auto-increment id is well within `Number.MAX_SAFE_INTEGER`).
//!
//! # wasm32-only
//!
//! Compiled only for `wasm32-unknown-unknown`; the native workspace build gates
//! this module out (see `lib.rs`), so `cargo check --workspace` stays browser-
//! dep-free.

use silent_core::questions::QwenNote;
use silent_notes::questions::{RerollOutcome, ScheduleOutcome, Scheduler, WorkerOutcome};
use silent_notes::{NoteExtractor, OpenQuestions, TriggerNote};

use silent_core::questions::{QuestionEvent, QuestionType};

use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Serialize a value to a `JsValue` via serde-json (a JSON string the glue
/// `JSON.parse`s). Matches the [`crate::diarization`] convention.
fn to_js_value<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(v).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

// ---------------------------------------------------------------------------
// WasmNoteEngine — NoteExtractor + OpenQuestions
// ---------------------------------------------------------------------------

/// The JSON shape one [`TriggerNote`] serializes to for the glue. camelCase so
/// it matches the index.html `{ category, text, triggerPhrase }` note objects
/// the `renderNote()` path already consumes (the `category` string matches the
/// `NoteCategory` snake-case keys: decisions/actions/keypoints/questions).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteJson {
    category: &'static str,
    text: String,
    trigger_phrase: String,
}

impl NoteJson {
    fn from_trigger(n: &TriggerNote) -> Self {
        // Reuse the serde rename of NoteCategory: serialize to its snake_case key.
        // The category strings are a fixed closed set, so map to &'static str.
        let category = match n.category {
            silent_core::notes::NoteCategory::Decisions => "decisions",
            silent_core::notes::NoteCategory::Actions => "actions",
            silent_core::notes::NoteCategory::Keypoints => "keypoints",
            silent_core::notes::NoteCategory::Questions => "questions",
            // The enum is #[non_exhaustive]; an unknown variant is not produced
            // by NoteExtractor, but we must handle it to compile. Emit empty so
            // the UI's renderNote() no-ops (its `_noteSectionIds` returns null).
            _ => "",
        };
        NoteJson {
            category,
            text: n.text.clone(),
            trigger_phrase: n.trigger_phrase.clone(),
        }
    }
}

/// Browser-facing live notes surface: the trigger extractor + open-question
/// tracker (Appendix A rows 16, 18).
///
/// # Lifecycle (mirrors the index.html `NoteEngine` + `OpenQs`)
///
/// The glue drives the EXACT live-pipeline call order so parity holds:
///
/// ```text
/// per final transcript line (trigger detection on):
///   const notes    = engine.analyze(line);   // categorized trigger notes
///   const resolved = engine.consider(line);  // open-question ids resolved (BEFORE the add loop)
///   for (const id of resolved) strikeThroughQuestion(id);
///   for (const note of notes) {
///     const id = await db.notes.add({ ...note });   // JS owns the id
///     renderNote({ ...note, id });
///     if (note.category === 'questions') engine.addQuestion(id, note.text);
///   }
/// at stop:
///   const flushed = engine.flush();
///   for (const note of flushed) { const id = await db.notes.add(...); renderNote(...); }
///   // flushed questions are NOT added to the open-question tracker (parity).
/// open-questions counter = engine.openCount();
/// ```
///
/// Pure policy: no model, no DOM, no I/O. The storage id is assigned by JS
/// (`db.notes.add`), exactly as today — the wasm object owns only the trigger
/// regexes, the sentence buffer, and the open-question keyword overlap.
#[wasm_bindgen]
pub struct WasmNoteEngine {
    extractor: NoteExtractor,
    open_qs: OpenQuestions,
}

impl Default for WasmNoteEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmNoteEngine {
    /// Create a fresh note engine (index.html `new NoteEngine()` +
    /// `OpenQs.reset()`).
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        Self {
            extractor: NoteExtractor::new(),
            open_qs: OpenQuestions::new(),
        }
    }

    /// Categorize a final transcript line (`NoteEngine.analyze`). Returns a JSON
    /// array of `{ category, text, triggerPhrase }` note objects — the SAME
    /// shape the index.html `renderNote` / `db.notes.add` paths already consume.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure (should not occur
    /// for these well-typed structs).
    pub fn analyze(&mut self, text: &str) -> Result<JsValue, JsError> {
        let notes: Vec<NoteJson> = self
            .extractor
            .analyze(text)
            .iter()
            .map(NoteJson::from_trigger)
            .collect();
        to_js_value(&notes)
    }

    /// Consider a transcript line as a potential answer to open questions
    /// (`OpenQs.consider`). Skips lines that are themselves questions. Returns a
    /// JSON array of the note ids newly resolved by this line (as JS numbers).
    /// The UI strikes those through (`q-resolved`). Called BEFORE the add loop,
    /// matching the index.html order.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[allow(
        clippy::cast_precision_loss,
        reason = "ids originate from db.notes.add (Dexie auto-increment integers, \
                  well within Number.MAX_SAFE_INTEGER = 2^53); the u64 → f64 \
                  coercion is exact for them and returns the JS Number the glue \
                  matches against the db ids it assigned"
    )]
    pub fn consider(&mut self, text: &str) -> Result<JsValue, JsError> {
        let resolved: Vec<f64> = self
            .open_qs
            .consider(text)
            .into_iter()
            .map(|id| id as f64)
            .collect();
        to_js_value(&resolved)
    }

    /// Register a newly-detected open question (`OpenQs.add(id, text)`). `id` is
    /// the db.notes id the JS assigned (a JS number); it is stored as `u64` for
    /// the resolution match and echoed back unchanged by [`Self::consider`].
    ///
    /// Only call for notes whose category is `questions` (the index.html guard
    /// `if (note.category === 'questions') OpenQs.add(...)`).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "id is a db.notes.add Dexie auto-increment integer (finite, \
                  non-negative, < 2^53); the guarded f64 → u64 cast is exact. \
                  Non-finite / negative inputs cannot occur from db.notes but are \
                  clamped to 0 defensively rather than panicking"
    )]
    pub fn add_question(&mut self, id: f64, text: &str) {
        let id = if id.is_finite() && id >= 0.0 {
            id as u64
        } else {
            0
        };
        self.open_qs.add(id, text);
    }

    /// Force-flush the trailing sentence buffer at stop (`NoteEngine.flush`).
    /// Returns the same JSON note-array shape as [`Self::analyze`]. Flushed
    /// question notes are NOT added to the open-question tracker (the glue must
    /// not call [`Self::add_question`] for flush results — matching index.html,
    /// which never calls `OpenQs.add` in the stop flush loop).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn flush(&mut self) -> Result<JsValue, JsError> {
        let notes: Vec<NoteJson> = self
            .extractor
            .flush()
            .iter()
            .map(NoteJson::from_trigger)
            .collect();
        to_js_value(&notes)
    }

    /// The number of still-open (unresolved) questions
    /// (`OpenQs._updateCount`'s `open`). This is the value the Open Questions
    /// section counter shows.
    #[wasm_bindgen(js_name = openCount)]
    #[must_use]
    pub fn open_count(&self) -> u32 {
        self.open_qs.open_count()
    }

    /// The texts of the still-open questions, in insertion order
    /// (`OpenQs.openTexts`). Used by the stop-time question recap (the JS read
    /// them from the DOM; here the tracker owns them, giving the identical
    /// result without a DOM query).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = openTexts)]
    pub fn open_texts(&self) -> Result<JsValue, JsError> {
        to_js_value(&self.open_qs.open_texts())
    }

    /// Reset both the extractor buffer and the open-question tracker for a new
    /// meeting (index.html `this.noteEngine = new NoteEngine()` + `OpenQs.reset()`).
    pub fn reset(&mut self) {
        self.extractor = NoteExtractor::new();
        self.open_qs.reset();
    }
}

// ---------------------------------------------------------------------------
// WasmQuestionScheduler — the SmartQ teleprompter scheduler
// ---------------------------------------------------------------------------

/// A flattened generate request for the glue: the worker needs `request_id`,
/// `window`, and `kind` directly (no serde tag envelope to unwrap). `kind` is
/// the `QuestionType` snake-case key (`"clarify"` …) the glue maps to a system
/// prompt + chip label.
#[derive(serde::Serialize)]
struct RequestJson {
    request_id: u32,
    window: String,
    kind: &'static str,
}

/// A flattened ready-question for the glue: the text, the type key, and the
/// badge flag.
#[derive(serde::Serialize)]
struct ReadyJson {
    text: String,
    kind: &'static str,
    badge: bool,
}

/// The `QuestionType` snake-case key (matches the ts-rs / serde rename and the
/// index.html `SmartQ.TYPES` keys).
#[allow(
    clippy::match_same_arms,
    reason = "the explicit Clarify arm and the #[non_exhaustive] catch-all share \
              a body by design — an unknown future type defaults to the first \
              rotation key; keeping Clarify explicit documents the mapping"
)]
fn question_type_key(t: QuestionType) -> &'static str {
    match t {
        QuestionType::Clarify => "clarify",
        QuestionType::Risk => "risk",
        QuestionType::Followup => "followup",
        QuestionType::Coverage => "coverage",
        QuestionType::Deepen => "deepen",
        _ => "clarify",
    }
}

/// Extract the flattened request fields from a `QuestionEvent::GenerateRequest`.
/// Other variants cannot reach here (the scheduler only returns
/// `GenerateRequest` from `accumulate`/`reroll`/`on_worker_result` retry); an
/// unexpected variant yields an empty request the glue treats as a no-op.
fn request_json(event: QuestionEvent) -> RequestJson {
    match event {
        QuestionEvent::GenerateRequest {
            request_id,
            window,
            kind,
        } => RequestJson {
            request_id,
            window,
            kind: question_type_key(kind),
        },
        _ => RequestJson {
            request_id: 0,
            window: String::new(),
            kind: "clarify",
        },
    }
}

/// Extract the flattened ready fields from a `QuestionEvent::QuestionReady`.
fn ready_json(event: QuestionEvent) -> ReadyJson {
    match event {
        QuestionEvent::QuestionReady { text, kind, badge } => ReadyJson {
            text,
            kind: question_type_key(kind),
            badge,
        },
        _ => ReadyJson {
            text: String::new(),
            kind: "clarify",
            badge: false,
        },
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ScheduleJson {
    Accumulated,
    Generate {
        request: RequestJson,
        /// `true` when a reroll expanded a minimized bar — the glue then also
        /// applies the expand (the JS `rerollSmartQ` toggles minimize first).
        expanded: bool,
    },
}

/// The JSON shape returned by [`WasmQuestionScheduler::on_worker_result`].
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorkerResultJson {
    /// The reply is unique (or the final attempt): render the question.
    Ready { event: ReadyJson },
    /// The reply duplicated a recent question: re-issue `request` to the worker.
    Retry { request: RequestJson },
    /// The reply was superseded (reset / newer generation) — drop it.
    Superseded,
}

/// Map a non-reroll [`ScheduleOutcome`] to the glue JSON (no expand).
fn schedule_to_json(outcome: ScheduleOutcome) -> ScheduleJson {
    match outcome {
        ScheduleOutcome::Accumulated => ScheduleJson::Accumulated,
        ScheduleOutcome::Generate { request } => ScheduleJson::Generate {
            request: request_json(request),
            expanded: false,
        },
    }
}

/// Browser-facing smart-question scheduler (Appendix A rows 21, 22).
///
/// Owns the rolling transcript window, timing/char gates, type rotation, dedup
/// ring, and the minimize/badge state. Issues `GenerateRequest`s; the
/// `question-worker.js` Qwen worker executes them. Determinism: the clock
/// (`now_ms`) and the model output (`on_worker_result`) are injected, exactly as
/// the `silent_notes::questions::Scheduler` requires.
#[wasm_bindgen]
pub struct WasmQuestionScheduler {
    scheduler: Scheduler,
}

impl Default for WasmQuestionScheduler {
    fn default() -> Self {
        Self::new(JsValue::NULL)
    }
}

#[wasm_bindgen]
impl WasmQuestionScheduler {
    /// Create a scheduler. `enabled_types` is an optional JSON array of the
    /// question-type keys to rotate (`["clarify","risk",...]`, the
    /// `settings.smartqTypes` subset). `null`/`undefined`/empty falls back to
    /// the full default rotation, matching `SmartQ._enabledTypes`.
    ///
    /// # Errors
    ///
    /// (none — invalid entries are ignored; an empty result falls back to the
    /// default rotation, never panics.)
    #[wasm_bindgen(constructor)]
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen exported constructors must take JsValue by value \
                  (the generated glue moves the argument across the boundary); \
                  parse_enabled_types borrows it"
    )]
    pub fn new(enabled_types: JsValue) -> Self {
        console_error_panic_hook::set_once();
        let types = parse_enabled_types(&enabled_types);
        Self {
            scheduler: Scheduler::new(&types),
        }
    }

    /// Accumulate a finalized transcript fragment at `now_ms` (`SmartQ.accumulate`
    /// → `_maybeGenerate`). Returns the schedule JSON the glue acts on.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn accumulate(&mut self, text: &str, now_ms: f64) -> Result<JsValue, JsError> {
        let outcome = self.scheduler.accumulate(text, ms(now_ms));
        to_js_value(&schedule_to_json(outcome))
    }

    /// Force a fresh generation regardless of the gates (`SmartQ.reroll` /
    /// `rerollSmartQ`). Expands the bar if it was minimized; the returned
    /// `expanded` flag tells the glue to emit the expand (clear `has-new`).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn reroll(&mut self, now_ms: f64) -> Result<JsValue, JsError> {
        let RerollOutcome { expanded, outcome } = self.scheduler.reroll(ms(now_ms));
        let json = match outcome {
            ScheduleOutcome::Accumulated => ScheduleJson::Accumulated,
            ScheduleOutcome::Generate { request } => ScheduleJson::Generate {
                request: request_json(request),
                expanded,
            },
        };
        to_js_value(&json)
    }

    /// Route a worker reply back into the scheduler
    /// (`SmartQ._generate`'s loop body). `request_id` correlates the prior
    /// `GenerateRequest`; `text` is the worker's raw question output. Returns
    /// `ready` (render), `retry` (re-issue), or `superseded` (drop).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onWorkerResult)]
    pub fn on_worker_result(&mut self, request_id: u32, text: &str) -> Result<JsValue, JsError> {
        let json = match self.scheduler.on_worker_result(request_id, text) {
            WorkerOutcome::Ready(event) => WorkerResultJson::Ready {
                event: ready_json(event),
            },
            WorkerOutcome::Retry(request) => WorkerResultJson::Retry {
                request: request_json(request),
            },
            WorkerOutcome::Superseded => WorkerResultJson::Superseded,
        };
        to_js_value(&json)
    }

    /// Toggle the teleprompter minimize/expand state (`toggleSmartQ`). Expanding
    /// clears the new-question badge. Returns a JSON
    /// `QuestionEvent::MinimizeChanged`.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = toggleMinimize)]
    pub fn toggle_minimize(&mut self) -> Result<JsValue, JsError> {
        let event = self.scheduler.toggle_minimize();
        to_js_value(&event)
    }

    /// Reset all scheduling state for a new meeting (`SmartQ.reset`). Bumps the
    /// generation epoch so any in-flight worker reply is dropped.
    pub fn reset(&mut self) {
        self.scheduler.reset();
    }

    /// Whether the teleprompter is currently minimized (the bar's `minimized`
    /// class). Used by the glue to mirror DOM state.
    #[wasm_bindgen(js_name = isMinimized)]
    #[must_use]
    pub fn is_minimized(&self) -> bool {
        self.scheduler.is_minimized()
    }

    /// Whether the new-question badge dot is raised (`has-new`).
    #[wasm_bindgen(js_name = hasNewBadge)]
    #[must_use]
    pub fn has_new_badge(&self) -> bool {
        self.scheduler.has_new_badge()
    }
}

/// Parse the optional `enabled_types` JSON array (string keys) into
/// [`QuestionType`]s, ignoring unknown keys. An empty/absent value yields an
/// empty `Vec`, which `Scheduler::new` treats as "use the full default rotation".
fn parse_enabled_types(value: &JsValue) -> Vec<QuestionType> {
    let Some(s) = value.as_string() else {
        return Vec::new();
    };
    let keys: Vec<String> = serde_json::from_str(&s).unwrap_or_default();
    keys.into_iter()
        .filter_map(|k| match k.as_str() {
            "clarify" => Some(QuestionType::Clarify),
            "risk" => Some(QuestionType::Risk),
            "followup" => Some(QuestionType::Followup),
            "coverage" => Some(QuestionType::Coverage),
            "deepen" => Some(QuestionType::Deepen),
            _ => None,
        })
        .collect()
}

/// Coerce a JS-number `now_ms` to the scheduler's `u64` clock. JS timestamps are
/// non-negative integers well within `u64`; clamp defensively.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "now_ms is a Date.now() / session-elapsed millisecond timestamp \
              (finite, non-negative, far below u64::MAX); the guarded f64 → u64 \
              cast is exact for it. Non-finite / negative inputs are clamped to 0"
)]
fn ms(now_ms: f64) -> u64 {
    if now_ms.is_finite() && now_ms >= 0.0 {
        now_ms as u64
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Qwen final-notes free functions (drop-in for the JS)
// ---------------------------------------------------------------------------

/// Parse Qwen note output (`parseQwenNotes`). Returns a JSON array of
/// `{ cat, text, topic }` — the SAME shape the index.html `generateFinalNotes`
/// aggregation consumes (`topic` may be `null`).
///
/// # Errors
///
/// Returns a `JsError` only on JSON serialization failure.
#[wasm_bindgen(js_name = parseQwenNotes)]
pub fn parse_qwen_notes(raw: &str) -> Result<JsValue, JsError> {
    let notes = silent_notes::qwen::parse_qwen_notes(raw);
    to_js_value(&notes)
}

/// Split a transcript into ~`target`-char chunks on sentence boundaries
/// (`chunkTranscript`). Returns a JSON array of strings.
///
/// # Errors
///
/// Returns a `JsError` only on JSON serialization failure.
#[wasm_bindgen(js_name = chunkTranscript)]
pub fn chunk_transcript(text: &str, target: usize) -> Result<JsValue, JsError> {
    let chunks = silent_notes::qwen::chunk_transcript(text, target);
    to_js_value(&chunks)
}

/// Drop near-duplicate notes (`dedupeNotes`). Input/output is the JSON
/// `{ cat, text, topic }` array shape (same as [`parse_qwen_notes`]); call after
/// aggregating every chunk's parsed notes.
///
/// # Errors
///
/// Returns a `JsError` on JSON (de)serialization failure (an ill-formed input
/// array is a loud failure, not a silent drop).
#[wasm_bindgen(js_name = dedupeNotes)]
pub fn dedupe_notes(notes_json: &str) -> Result<JsValue, JsError> {
    let notes: Vec<QwenNote> = serde_json::from_str(notes_json).map_err(to_js_err)?;
    let deduped = silent_notes::qwen::dedupe_notes(notes);
    to_js_value(&deduped)
}

/// The final-notes chunking `generateFinalNotes` performs:
/// `target = max(500, ceil(len/18))`, then `chunkTranscript(...).slice(0, 22)`.
/// Returns a JSON array of the per-chunk worker inputs.
///
/// # Errors
///
/// Returns a `JsError` only on JSON serialization failure.
#[wasm_bindgen(js_name = finalNotesChunks)]
pub fn final_notes_chunks(transcript: &str) -> Result<JsValue, JsError> {
    let chunks = silent_notes::qwen::final_notes_chunks(transcript);
    to_js_value(&chunks)
}

/// Clean one per-type recap group's raw model output (`generateQuestionRecap`
/// line cleaning): strip numbering/bullets, one edge quote, drop ≤6-char lines,
/// dedup, keep the first 3. Returns a JSON array of cleaned question strings.
///
/// # Errors
///
/// Returns a `JsError` only on JSON serialization failure.
#[wasm_bindgen(js_name = recapCleanGroup)]
pub fn recap_clean_group(raw: &str) -> Result<JsValue, JsError> {
    let cleaned = silent_notes::questions::recap_clean_group(raw);
    to_js_value(&cleaned)
}
