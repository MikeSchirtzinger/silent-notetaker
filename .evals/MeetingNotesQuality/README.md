# Meeting Notes Quality benchmark

This is a descriptive benchmark for the fixed meeting transcripts in `fixtures/`.
It measures grounded concept coverage, agenda reconciliation, usable structure,
and explicit non-invention checks. It does not estimate latent capability, item
difficulty, discrimination, or generalize beyond the frozen item bank.

Run deterministic validation:

```bash
node .evals/MeetingNotesQuality/validate-benchmark.mjs
node --test .evals/MeetingNotesQuality/score-output.test.mjs
```

Run the real browser model at `dev/final-notes-benchmark.html?autorun=1`. A run
counts only when it records Transformers.js as the backend, a Qwen model id,
raw model output for every item, and `mock: false` / `fallback: false`.
