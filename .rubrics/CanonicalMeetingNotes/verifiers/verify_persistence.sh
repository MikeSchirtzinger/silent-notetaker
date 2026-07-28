#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
rg -q "await storage\\.saveFinalNotes\\(this\\.meetingId, markdown\\)" index.html
rg -q "meeting\\.finalNotes \\|\\| exportsEngine\\.historyReplayMarkdown" index.html
rg -q "pub async fn save_final_notes" crates/silent-storage/src/writer.rs
rg -Fq 'set(&obj, "finalNotes"' crates/silent-storage/src/writer.rs
cargo test -p silent-core storage::tests::meeting_serde_roundtrip_with_end_time --quiet
