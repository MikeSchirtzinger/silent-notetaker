# Wiring hand-off notes — crate-policy fanout (generated from workflow wauzd0ruw)

## Integration report

Everything is integrated and clean. Here is my final report.

---

# Integration Complete — board o-rust-refactor-20260604

**Integration branch:** `wf/o-rust-refactor-integration` (worktree: `/Users/mike/dev/silent-notetaker`)
**Target advanced:** `rust-refactor` fast-forwarded to `cbb7725` (all 6 branches merged, `--no-ff`, history preserved)

## Merged branches (in required order, core-types-first)
| # | Branch | Result |
|---|---|---|
| 1 | wf/g1-note-extractor | clean (no conflict) |
| 2 | wf/g2-questions-qwen | conflicts resolved |
| 3 | wf/h1-session-machine | clean (git auto-union) |
| 4 | wf/h2-storage-migration | conflicts resolved |
| 5 | wf/h3-history-exports | conflicts resolved + **2 type collisions reconciled** |
| 6 | wf/i1-voxtral-recycle | clean (no conflict) |

## Conflicts resolved (file + how)
- **g2** `silent-core/src/lib.rs` — UNION module decls (`notes`+`questions`) and ts_bindings imports/export!/expected-files lists.
- **g2** `silent-notes/src/lib.rs` — UNION doc comment + module decls (`extractor`+`questions`+`qwen`), kept g1's `pub use extractor::*`.
- **g2** `silent-notes/Cargo.toml` — merged description; kept g1's runtime `regex` dep + both branches' serde/serde_json dev-deps.
- **g2/h2/h3** `Cargo.lock` — resolved per-crate dep lists then **regenerated via cargo check** (native + `--target wasm32`) to reconcile against merged manifests; verified all transitive trees present (regex/aho-corasick from g1; indexed_db_futures/web-sys/serde-wasm-bindgen chain from h2).
- **h2** `silent-core/src/lib.rs` — UNION `session`+`storage` modules (ts_bindings storage block auto-merged).
- **h3** `silent-storage/src/lib.rs` — kept all of h2's content (backup/reader/migrate/summary/wasm_api) + ADDED h3's `pub mod search;`.
- **h3** `silent-storage/Cargo.toml` — kept h2's full dep set (serde/serde_json/thiserror + wasm deps + `[[test]] browser_idb`); h3's dev-deps were redundant (covered by `[dependencies]`).

## Type collisions reconciled to ONE definition (the load-bearing work)
Two `silent-core` types were independently defined by two branches each. Both reconciled, **call sites fixed, and proven byte-identical** (export_bindings regenerated with ZERO drift):
- **`TimestampMode`** — defined by h1 (in `commands.rs`, referenced by `SessionEvent::TimestampModeChanged`) AND h3 (in `timestamp.rs`). Reconciled to the canonical one in `commands.rs`; added h3's `CYCLE` const + `label()` method (alias of existing `as_str()`); `timestamp.rs` now `pub use crate::commands::TimestampMode`.
- **`NoteCategory`** — defined by g1 (in `notes.rs`, referenced by `NoteCommand`/`NoteEvent`/`NoteCounters`) AND h3 (in `export.rs`). Reconciled to the canonical one in `notes.rs`; moved h3's `ORDER` const + `markdown_header()` impl there; `export.rs` now `pub use crate::notes::NoteCategory`.

## Final gate table (`scripts/ci-local.sh`, exit 0 = "All hard gates passed")
| Gate | Result |
|---|---|
| fmt (`--all --check`) | **PASS** |
| check (`--workspace --all-targets`) | **PASS** |
| test (`--workspace --all-targets`) | **PASS** (0 failures; silent-core 116+golden, silent-notes 20+9+3+4, silent-storage 9+9+search, silent-inference voxtral) |
| clippy (`--workspace --all-targets -D warnings`) | **PASS** |
| wasm32 clippy (silent-web/diarization/storage/inference/notes, lib + all-targets) | **PASS** (all exit 0) |
| supply-chain: cargo deny | **PASS** (advisories/bans/licenses/sources all ok) |
| supply-chain: cargo audit | SKIP (not installed on host) |
| boundary-fresh: export_bindings + `git diff --exit-code` | **PASS** (zero binding drift) |
| **browser-wasm: wasm-pack headless Chrome (browser_smoke)** | **PASS** (4 mel/decode tests, pinned chromedriver) |
| xtask model-audit | **PASS** (loud USER-GATE banners for license_verified=false + SenseVoice BLOCKED-ON-USER-GATE — expected, not my scope) |
| xtask gen-headers --check | **PASS** (_headers up to date) |
| link-check: lychee | SKIP (not installed on host) |

**One real failure found and resolved loudly:** `cargo deny` initially FAILED on **RUSTSEC-2024-0388 (`derivative` unmaintained)** — confirmed NEW vs baseline (baseline 0 occurrences), entering ONLY as a build-time proc-macro via `wasm_evt_listener → indexed_db_futures 0.6.4 → silent-storage` (the H2 storage migration's required IDB binding). Added a documented `[advisories] ignore` entry in `deny.toml` with full rationale + remediation note (build-only, wasm32 silent-storage only, never in the publishable nemotron-asr crate or any runtime path). Committed as cbb7725.

## Cleanup done
- All 6 `wf/*` worktrees removed (`git worktree remove`), `git worktree prune` run.
- All 6 merged `wf/*` feature branches deleted with `git branch -d` (clean-merge confirmed — no force needed).
- Leftover `worktree-wf_da4d5394-607-*` branches are orchestration bookkeeping (not in my scope; left intact). Untracked `.claude/` and `eval/js/package-lock.json` are pre-existing, outside scope.

---

## NOTES-FOR-WIRING digest (consolidated for the index.html wiring phase)

**Canonical silent-core boundary types (post-integration):**
- `TimestampMode` lives in `silent_core::commands` (re-exported at crate root). API: `CYCLE`, `next()`, `as_str()`==`label()`. TS: `"elapsed"|"clock"|"ago"`. Drive via `UiCommand::CycleTimestampMode`.
- `NoteCategory` lives in `silent_core::notes` (re-exported at crate root AND via `silent_core::export`). API: `ORDER`, `markdown_header()`. TS: `"decisions"|"actions"|"keypoints"|"questions"`.
- `BOUNDARY_CONTRACT_VERSION` = 1 (all additions `#[non_exhaustive]`-additive; the UI's wildcard switch arms cover new tags).

**G1 notes (silent-notes + silent_core::notes):** `NoteExtractor::analyze/flush`, `OpenQuestions::consider/add/open_count`. Live order: per final line `analyze` → `consider` (skips lines ending `?`, called BEFORE add loop) → add+render → if questions `open_qs.add(id,text)`; at stop `flush()` (flushed questions NOT added to OpenQs). `note_added.id`/`question_resolved.id` are `bigint` — coerce to JS number for db.notes.

**G2 questions/qwen (silent-notes):** `qwen::{parse_qwen_notes,chunk_transcript,dedupe_notes,final_notes_chunks}` (drop-in for the JS); NOTES_SYSTEM prompt + topic/HTML rendering stay in JS. `questions::Scheduler` externalizes the two-attempt dedup: `accumulate/reroll` → `GenerateRequest` (forward to question-worker.js); route reply via `on_worker_result(request_id,text)` → `Retry(next)` re-query / `Ready(QuestionReady{badge})` render. `recap_clean_group(raw)` per-type cleaning (preserved JS quirk: leading `* ` bullet NOT stripped).

**H1 session machine (silent_core::{SessionMachine,SessionConfig,SideEffect}):** drive `m.apply(&UiCommand, now_ms) -> Outcome{events,effects}` (`#[must_use]`). Machine decides cold-vs-warm from engine_loaded → read the SideEffect (`LoadEngineAndCapture` vs `ResumeCaptureNoReload` vs `StopCaptureKeepEngineLoaded`). `RunStopHooks(StopHooks{recluster,final_notes,question_recap,auto_summary})` gates each stop pass (summary modal always opens). Timer is host-driven: call `timer_text/current_duration_str/format_stamp/elapsed_ms`. Auto-title: on NewMeeting machine resets to "Untitled Meeting"+emits TitleChanged; HOST computes locale date string and sends back `SetTitle{title}`.

**H2 storage (silent-storage wasm exports):** `migrate_database(on_event)` — first wire the `{"tag":"backup_ready",...}` event to an `<a download>` BEFORE migration writes; then status/progress/completed(before==after). Sets `localStorage silentNotetaker_migrated_v3="1"` (one-time). Post-migration every `screenshot.image` is `Uint8Array` with `imageEncoding` marker: for `base64` do `TextDecoder().decode(...)` to recover the data-URL; for blob/bytes use `URL.createObjectURL(new Blob([...]))`. Live-capture render path unaffected. `read_database_summary()` for smoke/history.

**H3 history/exports (silent_core::{timestamp,export}, silent_storage::search):** `format_ms/format_elapsed/format_ago/format_clock/format_duration` (core has no Intl/TZ — host passes Date components for clock; `start_ms==0` falls back to ts). `notes_to_markdown/history_replay_markdown/executive_line/transcript_text/summary_markdown_with_ai` (DTO inputs `NoteRecord{time: already-formatted string}`/`TranscriptLine`/`AiNoteGroup`). Title fallback "Meeting Notes" (summary path's "Untitled Meeting" is the orchestrator's input). `search::{recent_meetings, search_history}` (HISTORY_LIMIT=50; pass notes++chunks as text_rows; empty query ⇒ all in order).

**I1 voxtral recycle (silent_inference::voxtral_recycle):** `VoxtralRecyclePolicy` (host-events `on_context_started/on_tokens/on_audio_advanced/on_decoded_text/on_context_end_text/request_stop`; pull-driver `poll(ring_write_abs, prompt_tokens) -> Option<HostCommand>`; on a Recycle immediately poll again for the seam context). Commands `StartContext/Recycle/Finalize` (serde tag="cmd"). `TextEvent::Partial`==onPartial / `::Final`==onFinal. Worker must keep transferable discipline (no PCM in serialized command). **Wiring TODO (I2):** add `AnyAsrEngine::JsHost(JsHostEngine)` in **silent-inference** (NOT silent-core — cycle), with the JsHostEngine struct + serde-wasm-bindgen glue in silent-web/silent-inference. Feed PerfMonitor (rows 34/35) from `SessionStats`/per-Recycle `ContextStats`.

---

## wf/g1-note-extractor

### Summary
PRD Phase 3 / board g1: ported the index.html note-trigger policy (NoteEngine + OpenQs + live counters) into Rust byte-identically, behind a typed silent-core boundary.

Branch wf/g1-note-extractor was created from rust-refactor HEAD (5f014b4) — NOT from the worktree's hn-prep checkout — because the Phase-1/2 workspace (crates/, the silent-notes stub, the silent-diarization tracker_ref.mjs golden-gen pattern this task was told to follow) lives on rust-refactor, not hn-prep.

What landed (2 commits):
1. silent-core: a new `notes` module with NoteCommand (UI->core: AnalyzeLine/Flush/SetTriggerDetection), NoteEvent (core->UI: NoteAdded/QuestionResolved/CountersChanged), NoteCategory, NoteCounters — all #[non_exhaustive] tagged enums following diarization.rs exactly; ts-rs bindings generated + committed (4 .ts files) and wired into the export_bindings test.
2. silent-notes: goldens/gen/notes_ref.mjs (DOM-free reimplementation of the EXACT JS, trigger regexes copied character-for-character so triggerPhrase == pattern.source) generating 9 golden fixtures; src/extractor.rs + src/extractor/triggers.rs (the NoteExtractor + OpenQuestions Rust port); tests/extractor_golden.rs proving byte-equality.

Byte-identity is proven adversarially: the stress_sweep fixture diffs Rust output vs the JS reference's exact emitted notes (id/category/text/byte-identical triggerPhrase), the four live counters after every step, and the final open-question texts — covering \b boundaries (disagreed must NOT match \bagreed\b), case-insensitivity, I'll/you'll contractions, weekday action pattern, metric keypoint, highest.leverage wildcard, and open-question resolution.

### Notes for wiring
API surface for the index.html wiring agent (Phase 3 row 16/18):

TYPES (crates/silent-core/src/notes.rs, TS bindings in crates/silent-core/bindings/):
- NoteCommand (UI->core), tagged { tag, payload }:
    analyze_line { text: string }        — send each FINAL transcript line when trigger detection is on
    flush                                — send once at Stop (flushes the trailing sentence buffer)
    set_trigger_detection { enabled }    — the Appendix A row 18 toggle (settingTriggers)
- NoteEvent (core->UI), tagged { tag, payload }:
    note_added { id: bigint, category: NoteCategory, text, trigger_phrase }
    question_resolved { id: bigint }     — strike-through the question note (q-resolved class)
    counters_changed { counters: NoteCounters }
- NoteCategory = "decisions" | "actions" | "keypoints" | "questions" (matches index.html section keys + _noteSectionIds map)
- NoteCounters = { decisions, actions, keypoints, questions } — questions is the OPEN count (OpenQs semantics).
  Note: ts-rs emits NoteEvent/NoteAdded.id and question_resolved.id as `bigint` (u64). The wiring shim should coerce to/from the JS number it uses for db.notes ids.

POLICY (crates/silent-notes, pub use NoteExtractor, OpenQuestions, TriggerNote, TriggerSet, counters):
- NoteExtractor::new(); .analyze(&str) -> Vec<TriggerNote>; .flush() -> Vec<TriggerNote>. Pure, no DOM, no model.
- OpenQuestions::new(); .reset(); .add(id:u64, text:&str); .consider(&str) -> Vec<u64> (ids newly resolved); .open_texts() -> Vec<String>; .open_count() -> u32.
- TriggerNote { category: NoteCategory, text: String, trigger_phrase: String }.

EXACT live-pipeline call order to preserve parity (from index.html addTranscript ~4343 + stop ~4085):
  per final line:  let notes = analyze(line); let resolved = open_qs.consider(line); for each note { id = db.notes.add(...); render; if questions -> open_qs.add(id, text) }
  at stop:         let notes = flush(); for each note { id = db.notes.add(...); render }  // flushed questions are NOT added to OpenQs
  questions counter = open_qs.open_count(); decisions/actions/keypoints counters = note counts per section.
consider() is called BEFORE the add loop, and it skips lines ending in '?'.

This module is the POLICY only; it does not own the storage ids (the orchestrator/silent-storage assigns them, exactly as db.notes.add does today). The Nemotron/Qwen adapters and smart-question scheduling (G2-G4) plug in alongside this, not through it.

---

## wf/g2-questions-qwen

### Summary
PRD Phase 3, board g2: ported the smart-question scheduling policy and the Qwen final-notes policy from index.html into pure Rust in crates/silent-notes, plus the QuestionCommand/QuestionEvent typed boundary in silent-core (ts-rs bindings). The worker (question-worker.js) stays the executor; the Rust policy emits typed commands/events. Followed the witness-everything / golden-first discipline: I wrote DOM-free JS reference generators (functions copied verbatim from index.html) that emit golden JSON FIRST, then ported to Rust, then proved byte-identical equality via golden tests.

NOTE ON WORKTREE STATE: this worktree was checked out at an old hn-prep commit (ddf49d9) with no crates/ scaffold. The actual Phase-1 workspace lives on rust-refactor (5f014b4). I created wf/g2-questions-qwen FROM rust-refactor (matching the sibling wf/g1-note-extractor base) and did all work there. Committed as d3600cc.

silent-core: new questions.rs module — QuestionCommand (accumulate/reroll/toggle/reset/recap/final-notes/worker-result), QuestionEvent (generate-request/question-ready/minimize-changed/recap-ready/final-notes-ready), QuestionType, RecapGroup, QwenNote — all #[non_exhaustive] tagged enums in the existing diarization-module style. Wired into lib.rs (module line + ts_bindings export). 5 new ts-rs bindings regenerated and committed.

silent-notes::questions: Scheduler — timing/char/window gates (MIN_INTERVAL 60s, MIN_CHARS 220, WINDOW_CHARS 1200, 120/40 floors), clarify/risk/followup/coverage/deepen rotation, two-attempt dedup ring (norm + last-8 eviction), reroll (force-gen + expand), toggle/minimize, new-question-badge state, generation-epoch supersession. Clock (now_ms) and model output (on_worker_result) injected so the policy is pure and command-log replayable. Plus recap_clean_group + norm.

silent-notes::qwen: parse_qwen_notes (TAG| / TAG: / TAG- / TAG– separators, TOPIC carry, NONE drop, 4/60/160-char floors+caps, <think> strip), chunk_transcript (sentence-boundary packing), dedupe_notes (>=60% keyword overlap, OPENQ_STOP stopwords, empty-kw exact-match path), final_notes_chunks (target = max(500, ceil(len/18)), .slice(0,22) cap). The three JS regexes are hand-rolled (no new dependency on the wasm target) with each source regex quoted at its site; goldens prove the hand-rolled matchers behave identically, including the apostrophe/word-boundary subtleties of the keyword regex.

### Notes for wiring
API SURFACE for the index.html wiring agent (all in crates/silent-notes; types in silent-core::questions):

QWEN FINAL-NOTES (silent_notes::qwen) — pure functions, drop-in replacements for the JS:
- parse_qwen_notes(raw: &str) -> Vec<QwenNote>            // == parseQwenNotes
- chunk_transcript(text: &str, target: usize) -> Vec<String>  // == chunkTranscript
- dedupe_notes(notes: Vec<QwenNote>) -> Vec<QwenNote>     // == dedupeNotes
- final_notes_chunks(transcript: &str) -> Vec<String>     // == the generateFinalNotes target=max(500,ceil(len/18)).slice(0,22)
QwenNote { cat: String ("decisions"|"actions"|"keypoints"|"questions"), text: String, topic: Option<String> }.
Wiring: generateFinalNotes still drives the Qwen worker, but the chunk list comes from final_notes_chunks(), and each worker reply is parsed by parse_qwen_notes() then the aggregate run through dedupe_notes() — identical output to today. The NOTES_SYSTEM prompt and the topic/Action-Items HTML rendering stay in JS (DOM); only the policy moved.

SMART-QUESTIONS (silent_notes::questions::Scheduler) — stateful policy:
- Scheduler::new(&[QuestionType]) / ::default()  (empty slice => full DEFAULT_TYPES rotation; pass the settings.smartqTypes subset here)
- accumulate(&mut self, text, now_ms) -> ScheduleOutcome { Accumulated | Generate{request: QuestionEvent::GenerateRequest{request_id, window, kind}} }
- reroll(&mut self, now_ms) -> RerollOutcome { expanded: bool, outcome: ScheduleOutcome }
- on_worker_result(&mut self, request_id, text) -> WorkerOutcome { Ready(QuestionEvent::QuestionReady{text,kind,badge}) | Retry(GenerateRequest) | Superseded }
- toggle_minimize() -> QuestionEvent::MinimizeChanged{minimized}
- reset()  // SmartQ.reset for new meeting
- accessors: is_minimized(), has_new_badge(), shown(), window_len(), recent_snapshot()

DRIVING MODEL (key for the wiring agent): the JS SmartQ did the two-attempt dedup loop INSIDE one async _generate. The Rust scheduler externalizes it: accumulate/reroll return a GenerateRequest (forward it to question-worker.js), and the worker's reply must be routed back via on_worker_result(request_id, text). If that returns Retry(next_request), send that to the worker too (this is the "rotate type + re-query on duplicate" behavior). When it returns Ready(QuestionReady{...}), render the question; `badge` is true when minimized (raise the dot). The window in GenerateRequest is already WINDOW_CHARS-capped and is the conditioning context for the worker's transcript prompt.

RECAP CLEANING: recap_clean_group(raw) -> Vec<String> reproduces generateQuestionRecap's per-type line cleaning (strip numbering/bullets, one edge quote, drop <=6-char lines, dedup, top 3). The per-type system prompts/labels (Clarifying/Devil's Advocate/...) and the modal HTML stay in JS; feed each type's raw model output through this fn. NOTE a faithfully-preserved JS quirk: a leading "* " bullet is NOT stripped (the JS regex ^[\s\-\d.)]+ omits '*'), so "* Will QA sign off?" stays as-is — this is intentional parity, not a bug.

GOLDEN MAINTENANCE: if the JS behavior ever changes, regenerate via `node crates/silent-notes/goldens/qwen/reference/qwen_reference.mjs` and `node crates/silent-notes/goldens/questions/reference/questions_reference.mjs`, then the golden tests flag any Rust divergence.

MERGE NOTE for g1-note-extractor (same crate): I only added `pub mod questions; pub mod qwen;` to silent-notes/lib.rs and updated its doc comment. g1 adds its own `mod extractor;` line — the doc-comment hunk may conflict trivially; module decls are independent. OPENQ_STOP stopword set is currently duplicated in my qwen.rs; if g1 also needs it (OpenQs tracking), consider a shared const in a future cleanup.

---

## wf/h1-session-machine

### Summary
PRD Phase 4 / board h1: ported index.html's recording-session logic (App.start/stop/newMeeting/tickTimer/updateTabUI) into a deterministic, browser-free state machine in silent-core::session. Branch wf/h1-session-machine created from rust-refactor (NOT hn-prep — the task depends on Phase C's silent-core scaffold, which only exists on rust-refactor; ddf49d9 is the merge-base so this was a clean fast-forward base). Committed as f0b6a05.

DELIVERED (crates/silent-core only — index.html and all other crates untouched):
- crates/silent-core/src/session.rs (NEW, ~1190 lines): SessionMachine pure value type. Owns Idle/Loading/Recording/Stopped transitions for start / stop / resume-without-reload / new-meeting; an engine_loaded flag making warm "Continue" a TYPED guarantee (LoadEngineAndCapture vs ResumeCaptureNoReload SideEffect) rather than a host guess; Mic/Tab source tracking; 120-char title clamp + trim/default; elapsed/clock/ago timestamp modes with byte-exact formatMs/formatStamp ports; stop-time hooks (recluster/final-notes/question-recap/auto-summary) as typed trigger-point events + RunStopHooks side effect (R2 law-vs-hands split). No clock, no I/O, no async — time is injected; serializable for command-log replay.
- commands.rs EXTENDED (not forked): UiCommand += SetTitle/AddTabAudio/RemoveTabAudio; SessionEvent += TitleChanged/SourcesChanged/TimestampModeChanged/StopHooks; new TimestampMode + StopHooks types.
- lib.rs: session module wired in, new types added to ts-rs export.
- ts-rs bindings regenerated: SessionEvent.ts + UiCommand.ts updated, StopHooks.ts + TimestampMode.ts new.

33 new deterministic tests cover every transition incl. resume-without-reload, explicit-resume, no-op guards (double-start, stop-while-idle/stopped, new-meeting-while-recording), idempotent tab toggles, tab-teardown-on-stop, tab-not-carried-across-resume, stop-hook config gating (incl. question_recap requiring both flags), timer projection (live→frozen, clock fallback), and command-log replay determinism.

### Notes for wiring
API SURFACE for the index.html wiring agent (all in silent-core, re-exported at crate root: SessionMachine, SessionConfig, SideEffect):

DRIVE LOOP: let mut m = SessionMachine::new() (or ::with_config(SessionConfig{ ai_final_notes, smart_questions, smartq_recap, auto_summary })). For each user action call m.apply(&UiCommand, now_ms) -> Outcome { events: Vec<SessionEvent>, effects: Vec<SideEffect> }. Outcome is #[must_use]. now_ms is the host's Date.now(); commands that don't need a clock ignore it. Forward every SessionEvent to the UI; execute every SideEffect in order.

UiCommand mapping (UI button → command):
- Start button (cold) / Continue button (warm) → StartRecording { title } (title = live #meetingTitle value). The machine itself decides cold-vs-warm from engine_loaded — the host does NOT need two code paths; just read the resulting SideEffect.
- Stop button → StopRecording
- explicit warm restart → ResumeRecording (equivalent to Continue; emits ResumeCaptureNoReload)
- New Meeting button → NewMeeting
- title input change → SetTitle { title } (clamps to 120, echoes back via TitleChanged — host should also keep maxlength=120 on the input)
- Share Tab / Remove Tab button → AddTabAudio / RemoveTabAudio (only valid while Recording; else a Notice no-op)
- timestamp cycle button → CycleTimestampMode

SideEffect → host action (the 'hands'):
- LoadEngineAndCapture = cold start: load selected engine (settings.model), then open getUserMedia mic + begin transcription (index.html's !canResume branch).
- ResumeCaptureNoReload = warm: engine already loaded; re-open capture + resume the streaming loop WITHOUT reloading the model (the canResume branch — Voxtral/Nemotron/Worklet resume paths).
- StopCaptureKeepEngineLoaded = tm.stop() + await trailing tail (e.g. _nemotronStopPromise); DO NOT null the engine (kept for Continue).
- AddTabAudio / RemoveTabAudio = tm.addSystemAudio()/removeSystemAudio().
- RunStopHooks(StopHooks{recluster, final_notes, question_recap, auto_summary}) = run each enabled pass: globalRecluster (DiarizationCommand::GlobalRecluster), generateFinalNotes (Qwen), generateQuestionRecap, summary pass. The summary MODAL always opens at Stop regardless; auto_summary gates the on-device/bridge summary request.

SessionEvent → UI render:
- StateChanged{state} → toggle Start/Stop/Continue/NewMeeting button visibility, recordingDot.active, timer.running per state.
- SourcesChanged{mic, tab} → show/hide #sourceMic and #sourceTab badges (row 6).
- TitleChanged{title} → set #meetingTitle.value (already clamped).
- TimestampModeChanged{mode} → "elapsed"|"clock"|"ago"; re-render stamps.
- StopHooks(...) → informational; the matching RunStopHooks effect does the work.
- Engine(EngineEvent), SpeakerLabel{...}, Notice{message} (toast), Error(AsrError) as before.

TIMER: the machine does NOT tick itself. Host keeps its 1s setInterval and on each tick calls m.timer_text(now_ms, clock) for the header (clock = host-formatted wall-clock string for clock mode, else None), m.current_duration_str(now_ms) for exports (always mm:ss), and m.format_stamp(ts_ms, now_ms, clock) per transcript line. m.elapsed_ms(now_ms) gives raw elapsed.

AUTO-TITLE (row 3): silent-core has no clock/locale, so it canNOT generate the "Wed, Jun 4 2:30 PM" string. On New Meeting the machine resets the stored title to "Untitled Meeting" and emits TitleChanged; the HOST must then compute its locale date/time string (the existing index.html toLocaleDateString/toLocaleTimeString code) and send SetTitle { title } to install it. Wire NewMeeting → (host computes auto-title) → SetTitle.

BOUNDARY_CONTRACT_VERSION is still 1 (all additions are #[non_exhaustive]-additive — no major bump needed). The UI's wildcard switch arm already covers the new event/command tags.

---

## wf/h2-storage-migration

### Summary
Productionized the a4 storage spike into crates/silent-storage + silent-core storage types (PRD Phase 4, board h2). silent-core gained a storage.rs module (the Dexie v2 SilentNotetaker schema as typed records + the migration/backup boundary events) with committed ts-rs bindings; silent-storage gained the proven two-phase IndexedDB reader, the zero-loss migration with export-backup-before-migrate, a native-testable JSON backup serializer, and a wasm-bindgen-test browser suite. Branched from rust-refactor (5f014b4); zero file overlap with the one commit rust-refactor advanced since, so it integrates cleanly.

Key correctness work beyond a literal port: the spike's synthetic fixture never exercised the partially-populated rows a REAL captured DB contains. Meeting.endTime is now Option<f64> because the shipping app writes `endTime: null, duration: 0` for in-progress meetings (index.html:3972) — a non-optional f64 would fail to deserialize null and LOSE that user's meeting, the exact failure this subsystem exists to prevent. Screenshot.analysis and Note.triggerPhrase default to "" when absent. The migration writes an `imageEncoding` marker alongside the normalized Uint8Array so a base64-string screenshot stays distinguishable from natively-binary bytes after both become Uint8Array (no representation ambiguity, no render-path breakage); verify_zero_loss asserts that original encoding survives.

### Notes for wiring
API SURFACE for the index.html wiring agent (wiring happens later, serially — I did NOT touch index.html):

silent-storage wasm-bindgen exports (built via wasm-pack into the silent-web/diarization-engine.js-style pkg):
- `migrate_database(on_event: Function) -> Promise<counts>`: runs the full Dexie v2 -> Rust zero-loss migration. `on_event` is called with a JSON STRING for each StorageEvent ({"tag":...,"payload":...}). The FIRST meaningful event the UI must handle is `{"tag":"backup_ready","payload":{object_url, filename, size_bytes, counts}}` — wire an <a download={filename} href={object_url}> so the user can save the backup BEFORE the migration writes (PRD Phase 4 exit criterion). Then `status_changed` (pending/awaiting_backup/migrating/complete/already_migrated), `progress` {done,total}, and finally `completed` {before, after} (before MUST equal after = zero loss) or `failed` {message}. Resolves to the after-migration StorageCounts JS object; rejects with a string on failure.
- `read_database_summary() -> Promise<summary>`: per-table arrays + screenshot metadata incl. an `encoding` tag (base64/blob/bytes/empty) + `screenshotBlobs` (Uint8Array[]). Used for smoke checks / history.

TypeScript types for the UI are generated in crates/silent-core/bindings/ (StorageEvent.ts, MigrationStatus.ts, Meeting.ts with `endTime: number | null`, Screenshot.ts, etc.). camelCase keys match the Dexie schema; the boundary event enums are snake_case-tagged exactly like DiarizationEvent/SessionEvent.

IMPORTANT post-migration render note: after migration EVERY screenshot.image is a Uint8Array, with a NON-INDEXED `imageEncoding` marker field ("base64"|"blob"|"bytes") recording the original. The Rust reader recovers it; the JS render path must reconstruct accordingly — for `base64`, do `new TextDecoder().decode(uint8array)` to get back the data: URL string for `<img src>`; for `blob`/`bytes`, use `URL.createObjectURL(new Blob([uint8array]))`. The current app's live capture render path (index.html:4905 addScreenshotThumbnail) is unaffected — it renders freshly-captured base64 strings, never reads migrated rows; the current history path only count()s screenshots, so nothing breaks today.

The migration sets localStorage `silentNotetaker_migrated_v3 = "1"` so it's a one-time no-op on re-run.

INTEGRATION: branch is based on rust-refactor@5f014b4; current rust-refactor tip (51c6fbb, a deploy-script/pkg-artifact change) has ZERO file overlap with my changes — merges cleanly. Cargo.lock added 27 transitive deps (indexed_db_futures 0.6.4 + futures chain) for silent-storage only.

---

## wf/h3-history-exports

### Summary
PRD Phase 4 / board h3 (history + exports policy) complete. Ported from index.html into Rust as pure, DOM-/Intl-free policy, with current JS behavior captured as DOM-free golden references FIRST, then proven byte-identical.

IMPORTANT BASING NOTE: my worktree HEAD was hn-prep (ddf49d9), which predates the cargo workspace. The workspace (crates/silent-core, silent-storage, etc.) only exists on rust-refactor (5f014b4), which my HEAD is a direct ancestor of. I created wf/h3-history-exports FROM rust-refactor (not bare HEAD) so the Phase-C-built crates my task targets are present — Phase H depends on Phase C. My two commits sit directly on top of rust-refactor.

silent-core (exporter + timestamp module):
- timestamp.rs: the three modes elapsed/clock/ago + format_ms + format_duration + TimestampMode enum (cycle order/labels). Reproduces JS arithmetic quirks exactly (Math.floor toward -inf, sign-preserving %, padStart-only-when-short) so negative elapsed deltas format identically ("-1:00", "-2:-59"). clock validated against real Intl en-US 12-hour output.
- export.rs: notes_to_markdown (timestamp-aware, empty-text filter), history_replay_markdown, executive_line (singular/plural), transcript_text (timestamp-aware), summary_markdown_with_ai (AI-notes append). NoteCategory enum owns section order.
- New ts-rs bindings (TimestampMode.ts, NoteCategory.ts) regenerated and committed; export_bindings test extended; no drift.

silent-storage (search module):
- search.rs: search_history + recent_meetings + HISTORY_LIMIT=50. "Fuzzy" captured verbatim = case-insensitive substring across title → notes → transcript chunks, newest-first order, 50-meeting window. Pure stdlib (serde only as test dev-dep). Only touched search.rs + one `pub mod search;` line in lib.rs + dev-deps, per h2 coexistence note.

All behavior pinned by JS-generated goldens (goldens/gen/*_ref.mjs) and proven equal in Rust golden tests.

### Notes for wiring
API SURFACE for the index.html wiring agent (all pure functions; the UI/silent-web supplies the inputs the DOM/Intl previously computed):

silent-core::timestamp (also re-exported at crate root):
- fn format_ms(ms: i64) -> String              // mm:ss, JS-quirk-faithful
- fn format_elapsed(ts_ms: i64, start_ms: i64) -> String   // start_ms==0 ⇒ falls back to ts (JS `|| tsMs`) ⇒ "00:00"
- fn format_ago(ts_ms: i64, now_ms: i64) -> String         // caller passes current time; core never reads a clock
- fn format_clock(hour: u8, minute: u8) -> String          // caller passes LOCAL Date#getHours()/getMinutes(); core has no Intl/TZ
- fn format_duration(ms: i64) -> String        // "Nm Ns" for the history list
- enum TimestampMode { Elapsed, Clock, Ago }; ::CYCLE, .next() (wraps; unknown⇒Elapsed), .label() ("elapsed"/"clock"/"ago"). Maps to UiCommand::CycleTimestampMode. TS binding: type TimestampMode = "elapsed"|"clock"|"ago".

silent-core::export (re-exported at crate root):
- struct NoteRecord { category: NoteCategory, text: String, time: Option<String> }  // `time` is the ALREADY-FORMATTED stamp string (.note-time textContent), not a number
- struct TranscriptLine { time: String, text: String }
- struct AiNoteGroup { label: String, items: Vec<AiNoteItem> }; AiNoteItem { chip: Option<String>, text: String }
- enum NoteCategory { Decisions, Actions, Keypoints, Questions } (snake_case serde; ::ORDER, .markdown_header()). TS binding: union of those 4 strings.
- fn notes_to_markdown(title, date, duration: &str, notes: &[NoteRecord], with_time: bool) -> String
- fn history_replay_markdown(title, date, duration: &str, notes: &[NoteRecord]) -> String   // always `- text`, no per-line stamps
- fn executive_line(duration: &str, notes: &[NoteRecord], total_words: u64) -> String
- fn transcript_text(lines: &[TranscriptLine], with_time: bool) -> String
- fn summary_markdown_with_ai(base_md: &str, ai_groups: &[AiNoteGroup]) -> String   // compose: base = notes_to_markdown(...); then this

silent-storage::search:
- type MeetingId = i64; struct MeetingRecord { id, title, start_time } ; struct TextRow { meeting_id, text } (one type for BOTH notes and transcript chunks — predicate is identical)
- const HISTORY_LIMIT: usize = 50
- fn recent_meetings(&[MeetingRecord], limit) -> Vec<&MeetingRecord>   // newest-first, capped
- fn search_history(meetings, text_rows, query: &str, limit) -> Vec<MeetingId>  // pass notes ++ chunks as text_rows; empty/whitespace query ⇒ all candidate ids in order

KEY CONTRACT POINTS: title fallback is "Meeting Notes" (notes/replay/exports) — note the summary path in JS uses "Untitled Meeting", which is the orchestrator's responsibility to pass in, not this module's. NoteRecord/TranscriptLine/AiNoteGroup are export-input DTOs (serde-deserializable; category strings match JS values); they intentionally do NOT have ts-rs bindings — only the user-facing TimestampMode and NoteCategory enums do.

H2 COEXISTENCE: I added exactly one line to silent-storage/src/lib.rs (`pub mod search;`) plus a doc tweak and serde/serde_json dev-deps. H2 (db modules) adding its own `pub mod` lines will produce at most a trivial lib.rs conflict resolvable by keeping both.

---

## wf/i1-voxtral-recycle

### Summary
PRD Phase 5 board i1: ported Voxtral's token/audio two-cap context recycle — the hardest-won bug fix in the app (index.html `_runVoxtralTranscription`, the 2026-05-29 RAM fix) — from a JS closure into a deterministic, natively unit-tested Rust policy module at crates/silent-inference/src/voxtral_recycle.rs (PRD R2; Appendix A row 10). Modeled as the JsHostEngine policy core per the b2 spike pattern: the policy owns the law and emits typed HostCommands; the transformers.js worker (later wiring in silent-web) is the executor and holds zero policy.

Ported exactly: (1) TOKEN CAP max_new_tokens=320 (>= boundary, generate returns), (2) AUDIO/TIME CAP MAX_CTX_SAMPLES=16000*45=45s (strict-> boundary, evaluated in the mel generator), with audio-cap-wins precedence when both trip. Seam semantics: a recycled context anchors at the CURRENT ring write position — never re-reads evicted audio, never skips ("continuous across seams"). In-place partial text: the printLen/sentenceBuffer machine (flushDecodedText + streamer.end) with the dotall-greedy /^(.*[.!?])\s*/s sentence boundary ported by hand (no regex dep), char-boundary-safe for Unicode. Bounded-context observability via ContextStats/SessionStats mirroring the Diag counters (loopIter/recycleCount/ctxLen/inputTokens).

22 deterministic tests (no browser, no mocks): each cap's decision point, recycle-preserves-context seam, per-context stats reset, stop/finalize idempotency, in-place partial-text segmentation, HostCommand/TextEvent serde wire-shape, and TWO 10-minute-session simulations (slow-token profile -> audio cap fires ~13x; dense-token profile -> token cap fires) each asserting bounded per-context growth (the sawtooth, not unbounded heap — PRD R9).

WRITE scope respected: only crates/silent-inference/ touched, plus the auto-updated Cargo.lock (two dep lines). index.html and all other crates untouched.

### Notes for wiring
API surface (crates/silent-inference, module `voxtral_recycle`), all serde-ready for the typed boundary:

CONFIG
- `RecycleConfig { max_new_tokens: u32, max_ctx_samples: u64 }`, `RecycleConfig::VOXTRAL_SHIPPING` (320 tokens / 720_000 samples = 45s), `Default`. In production these should come from registry data (Task I3), not be hardcoded.

POLICY DRIVER — `VoxtralRecyclePolicy`
- `new(RecycleConfig) -> Self`
- Host-event inputs (call as the worker reports): `on_context_started(prompt_tokens: u32)`, `on_tokens(n: u32)` (a streamer.put delta), `on_audio_advanced(anchor_abs: u64, consumed_abs: u64)`, `on_decoded_text(cumulative_decoded: &str) -> Vec<TextEvent>` (pass the worker's cumulative tokenizer.decode), `on_context_end_text() -> Vec<TextEvent>` (call when generate returns), `request_stop()`.
- Driver (pull-based, the outer-while): `poll(ring_write_abs: u64, prompt_tokens: u32) -> Option<HostCommand>`. Idle->StartContext, Running->Recycle-or-None, Stopped->one Finalize then None. On a Recycle, immediately poll again to open the seam context (the JS outer-while does this).
- Read-only: `config()`, `session_stats() -> SessionStats`, `current_stats() -> ContextStats`, `is_running()`.

COMMANDS (policy -> JS host; #[non_exhaustive], serde tag="cmd", snake_case)
- `StartContext { context: u32, anchor_abs: u64, max_new_tokens: u32 }` -> host calls model.generate with max_new_tokens, streams mel forward from anchor_abs (ring position).
- `Recycle { context: u32, reason: RecycleReason, stats: ContextStats }` -> host tears down current generate context.
- `Finalize { stats: Option<ContextStats> }` -> end of stream.
- `RecycleReason` (#[non_exhaustive]): TokenCap | AudioCap | Stop, serializes "token_cap"/"audio_cap"/"stop".

TEXT EVENTS (policy -> UI; serde tag="kind" content="text")
- `TextEvent::Partial(String)` = JS onPartial (overwrite live element in place), `TextEvent::Final(String)` = JS onFinal (promote, sentence complete).

WIRING NOTES FOR index.html / silent-web:
- This is the JsHostEngine policy half. The worker (transformers-host.js style, see b2 spike) must keep the transferable discipline for any audio/mel payload and NOT put PCM in the serialized command (spike finding).
- The wiring agent / I2 should add `AnyAsrEngine::JsHost(JsHostEngine)` in silent-inference (NOT silent-core — that would create a dependency cycle; silent-core's AnyAsrEngine doc comment already prescribes adding concrete variants in the home crate). The JsHostEngine struct that owns a VoxtralRecyclePolicy + the wasm-bindgen/serde-wasm-bindgen glue lands in silent-web / silent-inference, not silent-core.
- silent-inference now depends on serde (derive) and dev-dep serde_json. No browser deps were added — keep it that way; the serde-wasm-bindgen hop belongs in silent-web.
- The policy needs no ring handle: it works purely from reported positions (anchor_abs/consumed_abs), which is what keeps it browser-free. The host echoes the StartContext anchor_abs back in on_audio_advanced.
- Diag/PerfMonitor (Appendix A row 34/35) can be fed directly from SessionStats (recycles, token_cap_recycles, audio_cap_recycles, tokens_total, contexts_started) and per-Recycle ContextStats — these ARE the bounded-growth trail.

