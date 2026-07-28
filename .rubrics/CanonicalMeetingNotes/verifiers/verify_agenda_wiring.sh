#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
rg -q "storage\\.addMeeting\\(title \\|\\| 'Untitled Meeting', agenda, this\\._startEpoch\\)" index.html
rg -q "setAgendaLocked\\(true\\)" index.html
rg -q "pub async fn add_meeting\\(title: &str, agenda: &str, start_time: f64\\)" crates/silent-storage/src/writer.rs
rg -Fq 'set(&obj, "agenda"' crates/silent-storage/src/writer.rs
node --test --test-name-pattern="agenda" tests/final-notes-policy.test.mjs >/dev/null
