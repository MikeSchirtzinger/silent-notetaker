# Canonical Meeting Notes rubric

Goal: stopping a recorded meeting produces grounded, shareable whole-meeting
notes, optionally organized by the supplied agenda, and persists them for
history replay.

All criteria are programmatic. A real-model result counts only when the browser
run identifies the Transformers.js pipeline and records raw model output; mock
or fallback responses are rejected.

## Changelog

- v1: initial whole-meeting synthesis, agenda, persistence, settings, benchmark,
  and browser-proof contract.
- v2: repair malformed literal searches in C4/C5 and replace A1's shell pipeline
  with a deterministic cross-platform test counter.
- v3: repair C3 after real-model calibration showed that its required
  `OVERVIEW|` response seed causes Qwen3 0.6B repetition loops. C3 now verifies
  one unseeded natural synthesis call plus the grounded source-register gates.
