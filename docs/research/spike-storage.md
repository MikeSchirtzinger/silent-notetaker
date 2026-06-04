# Spike S4: Storage Migration — Dexie v2 → Rust WASM via `indexed_db_futures`

**Status:** COMPLETED  
**Date:** 2026-06-04  
**Agent:** a4-spike-storage  
**Spike directory:** `/Users/mike/dev/snt-spikes/a4-storage/`

---

## Result

**PASS — readback diff: EMPTY (zero data loss confirmed) across ALL THREE
real-world image encodings, including the migration-critical JS `Blob` case.**

The spike proves that Rust WASM can open a Dexie v2 `SilentNotetaker` database,
read all four tables, and return data bit-for-bit identical to the fixture —
for screenshots stored as a **base64 data-URL string** (the current `index.html`
capture path), an **actual JS `Blob`** (resolved via the two-phase async pass),
and a **`Uint8Array`** (the normalised layout).

```
[0] Building screenshot fixtures (base64 + Blob + Uint8Array)...
    screenshot 1: encoding=base64, expected 1039 bytes
    screenshot 2: encoding=blob,   expected 67 bytes
    screenshot 3: encoding=bytes,  expected 69 bytes
[1] Dexie db.version(2) → actual IDB version: 20
[1] CONFIRMED: Dexie version multiplier = 10 (user 2 × 10 = IDB 20).
[1] Screenshot 1 stored image JS type: String     (expected encoding: base64)
[1] Screenshot 2 stored image JS type: Blob        (expected encoding: blob)
[1] Screenshot 3 stored image JS type: Uint8Array  (expected encoding: bytes)
[2] Rust readback: meetingCount=2, chunkCount=4, noteCount=4, screenshotCount=3
    totalBlobBytes=1175
[3] deep-equal:
    meetings: PASS
    transcriptChunks: PASS
    notes: PASS
    image[0] (base64): 1039 bytes match
    image[1] (blob):   67 bytes match
    image[2] (bytes):  69 bytes match
    screenshots: PASS
RESULT: PASS -- readback diff EMPTY, zero data loss confirmed.

--- IDB trap verification (raw IDBFactory, independent of Rust) ---
TRAP CONFIRMED: reusing tx after a non-IDB await throws -> TransactionInactiveError
TWO-PHASE CONFIRMED: Blob resolved after tx closed -> 67 bytes
```

The `encoding` field returned by Rust proves each storage layout was actually
exercised (not just the easy `Uint8Array` path). The independent raw-`IDBFactory`
check proves the transaction-auto-close trap is real and that the two-phase
design defeats it.

---

## Scope

Reads the `SilentNotetaker` Dexie v2 schema from `index.html` ~line 1967:

```javascript
const db = new Dexie('SilentNotetaker');
db.version(2).stores({
  meetings: '++id, title, startTime, endTime, duration',
  transcriptChunks: '++id, meetingId, timestamp, text, isFinal',
  notes: '++id, meetingId, category, text, timestamp, triggerPhrase',
  screenshots: '++id, meetingId, timestamp, image, width, height, analyzed, analysis',
});
```

---

## Key Finding 1: Dexie Version Multiplier

**Dexie multiplies the user-visible version by 10 before calling `IDBFactory.open()`.**

Verified in Dexie source (`dexie-open.ts`):
```javascript
let nativeVerToOpen = Math.round(db.verno * 10);
indexedDB.open(dbName, nativeVerToOpen);
```

Verified in-browser via raw `IDBFactory`:
```
IDB raw version: 20    (Dexie db.version(2) × 10 = 20)
```

**Implication for `silent-storage` Phase H:**

When opening the database from Rust, do NOT call `Database::open("SilentNotetaker").with_version(2)` — that would trigger a downgrade error or an unwanted upgrade. Instead:

```rust
// Correct: open at current version, no upgrade handler.
let db = Database::open("SilentNotetaker")
    .build()?
    .await?;
```

If a schema migration is needed (Phase H2), open with `with_version(20 * new_dexie_version)` and provide an `on_upgrade_needed` handler. For zero-loss read-only migration, `with_version` is unnecessary.

**Alternative approach for Phase H:** if Rust takes full ownership of the DB schema, delete the Dexie DB and recreate it via Rust. Back up first with `export-backup` (PRD Phase 4 exit criterion).

---

## Key Finding 2: `indexed_db_futures` 0.6.x API

### API overview

The crate uses a builder pattern throughout:

```rust
// Open (no upgrade):
let db = Database::open("name").build()?.await?;

// Read-only transaction:
let tx = db.transaction(["storeName"])
    .with_mode(TransactionMode::Readonly)
    .build()?;
let store = tx.object_store("storeName")?;

// Cursor-based full scan with serde deserialization:
if let Some(cursor) = store.open_cursor().build()?.await? {
    let mut stream = cursor.stream_ser::<MyType>(); // requires cursors+streams+serde features
    while let Some(value) = stream.try_next().await? {
        records.push(value);
    }
}

// Commit (drops without commit = auto-abort):
tx.commit().await?;
```

### Required Cargo features

```toml
indexed_db_futures = { version = "0.6", features = ["cursors", "streams", "serde"] }
```

- `cursors`: enables `open_cursor()` on object stores
- `streams`: enables `cursor.stream_ser::<T>()` and `cursor.stream::<JsValue>()`
- `serde`: enables serde-based deserialization via `TryFromJs`

### Serde field name matching

Dexie stores JS objects with camelCase keys. Rust struct field names must match exactly using `#[serde(rename = "...")]`:

```rust
#[derive(Serialize, Deserialize)]
pub struct TranscriptChunk {
    pub id: u32,
    #[serde(rename = "meetingId")]  // Dexie key is camelCase
    pub meeting_id: u32,
    #[serde(rename = "isFinal")]
    pub is_final: bool,
    // ...
}
```

### `get_all` vs cursor

`ObjectStore::get_all()` exists but returns a flat `Vec<JsValue>`. The cursor approach with `stream_ser` is more ergonomic and memory-efficient for large tables. Both work.

---

## Key Finding 3: The `image` field has THREE real-world encodings — all proven

**This is the most important implementation detail for Phase H. All three are now
proven zero-loss, including the migration-critical `Blob` case.**

### What `index.html` actually stores today (corrected)

The capture path at `index.html:2959` does NOT store a `Blob`. It stores a
**base64 data-URL string**:

```javascript
this.captureCanvas.toBlob((blob) => {
  const reader = new FileReader();
  reader.onloadend = () => {
    const base64 = reader.result;        // 'data:image/jpeg;base64,...'
    db.screenshots.add({ ..., image: base64, ... });  // STRING, not Blob
  };
  reader.readAsDataURL(blob);
}, 'image/jpeg', 0.7);
```

Readback (`index.html:5049`) feeds the string straight into `<img src="${base64}">`.
So the current shipping representation is a **string**, and migrating it losslessly
means preserving the exact string bytes.

### Why a representative DB must still cover `Blob`

`Blob` is the migration-critical case for two reasons:
1. The `screenshots.image` Dexie field is untyped; a future or alternate capture
   path that stores `canvas.toBlob()` output directly (the obvious naive design)
   produces a `Blob`, and the migration must not lose that user's screenshots.
2. Resolving a `Blob` from inside an IDB read is the classic IDB transaction trap;
   proving it works is the whole point of de-risking storage.

The spike therefore exercises **all three**: base64 string (current), `Blob`
(migration-critical), and `Uint8Array` (normalised). Every one reads back byte-exact.

### The transaction-auto-close trap — confirmed empirically

A JS `Blob` exposes its bytes only via `blob.arrayBuffer()`, which returns a
`Promise`. IndexedDB transactions auto-commit (close) the moment control returns to
the event loop with no pending IDB request. So if you `await blob.arrayBuffer()`
while the transaction is open, the transaction closes underneath you, and the next
cursor `continue()` / store access throws `TransactionInactiveError`.

Verified independently in-browser against raw `IDBFactory` (not via Rust):

```
TRAP CONFIRMED: reusing tx after a non-IDB await throws -> TransactionInactiveError
TWO-PHASE CONFIRMED: Blob resolved after tx closed -> 67 bytes
```

A `Blob` handle stays valid after its transaction closes (it is an independent JS
object, not bound to the cursor), which is exactly what makes the two-phase approach
work.

### The proven solution: strict two-phase read (implemented in the spike)

`read_screenshots_two_phase()` in `src/lib.rs`:

```rust
// Phase 1: transaction HELD. Scan cursor; pull scalars; stash live Blob handles.
//          NEVER await a non-IDB promise here.
let tx = db.transaction(["screenshots"]).with_mode(Readonly).build()?;
let store = tx.object_store("screenshots")?;
if let Some(cursor) = store.open_cursor().build()?.await? {
    let mut stream = cursor.stream::<JsValue>();
    while let Some(raw) = stream.try_next().await? {        // only IDB awaits
        partials.push(extract_partial_screenshot(&raw)?);  // Blob handle stashed
    }
}
tx.commit().await?;                                        // tx closes here

// Phase 2: NO transaction held. Now it is safe to await Blob promises.
for p in partials {
    let image = if let Some(blob) = p.pending_blob {
        let ab = JsFuture::from(blob.array_buffer()).await?; // safe — no tx open
        Uint8Array::new(&ab).to_vec()
    } else { p.image };
    // ...
}
```

The classification in `extract_partial_screenshot()` resolves the three encodings:

```rust
let (image, encoding, pending_blob) = if let Some(s) = image_val.as_string() {
    (s.into_bytes(), ImageEncoding::Base64String, None)     // current app
} else if image_val.is_instance_of::<Uint8Array>()
       || image_val.is_instance_of::<js_sys::ArrayBuffer>() {
    (Uint8Array::new(&image_val).to_vec(), ImageEncoding::Bytes, None)
} else if image_val.is_instance_of::<web_sys::Blob>() {
    let blob: web_sys::Blob = image_val.unchecked_into();
    (Vec::new(), ImageEncoding::Blob, Some(blob))           // Phase-2 resolved
} else {
    (Vec::new(), ImageEncoding::Empty, None)
};
```

### Recommended Phase H strategy

1. **Read path** (`silent-storage`): use the two-phase reader above. It is encoding-
   agnostic and loses nothing for base64, Blob, or Uint8Array.
2. **Normalise on migration write**: re-store every screenshot's `image` as a
   `Uint8Array` (decode base64 to its JPEG bytes, or keep the data-URL string —
   pick ONE canonical form; `Uint8Array` of the decoded image is the leanest and
   makes future reads single-phase). Whatever is chosen, do it once, gated behind
   the export-backup (PRD Phase 4 exit criterion).
3. **New writes** from the Rust-owned capture path store `Uint8Array` directly, so
   post-migration reads never need Phase 2 at all.

The two-phase reader remains the safe general tool even after normalisation, because
older un-migrated DBs may still contain Blobs.

---

## Key Finding 4: `indexed_db_futures` API Gaps

### Gap 1: No built-in `getAll` returning `Vec<T>` serde-direct

`store.get_all()` returns `GetAll<'_, Record, T, V>` which requires `TryFromJs`, not
`serde::Deserialize`. For serde types you must use the cursor path.

**Workaround:** cursor + `stream_ser::<T>()` works well and is what the spike uses.

### Gap 2: No `Blob` ↔ `Vec<u8>` conversion (handled, not blocking)

The crate has no `Blob::read_bytes()` or similar, and its cursor stream is
synchronous so it cannot resolve a `Blob`'s async `arrayBuffer()` inline.

**Solution (proven):** the two-phase reader in Key Finding 3 — stash the `Blob`
handle during the cursor scan, commit the transaction, then resolve each `Blob`
via `JsFuture::from(blob.array_buffer())` afterward. This is not a workaround that
weakens correctness; it is the only correct way to read Blobs from IDB in any
language, because the IDB transaction-auto-close trap (Gap 6) is universal.

### Gap 3: Transaction auto-abort semantics differ from JS

In `indexed_db_futures`, dropping a transaction without calling `.commit()` **aborts it**.
This is opposite to standard IndexedDB behavior where transactions auto-commit. The crate
does this intentionally to allow `?`-operator propagation. Always call `tx.commit().await?`
after writes (and after reads where you want the data to be considered committed).

### Gap 6: IDB transaction auto-closes across non-IDB awaits (universal trap)

Not specific to `indexed_db_futures` — it is an IndexedDB spec behavior — but it
shapes the Rust design. Awaiting any non-IDB future (e.g. `Blob.arrayBuffer()`,
`fetch`, a timer) while a transaction is open lets the transaction commit/close;
the next IDB operation on it throws `TransactionInactiveError`. **Confirmed
empirically in this spike** (see Key Finding 3). Rule: never hold an IDB transaction
across a non-IDB `await`. Collect everything you need from IDB in one synchronous
burst of IDB requests, commit, then do async work.

### Gap 4: `with_version` conflicts with Dexie's ×10 multiplier

Opening with `with_version(2)` will cause a version-change event (downgrade or spurious
upgrade) if the DB was created by Dexie at version 20. Omit `with_version` for read-only
access.

### Gap 5: No cursor count / limit

No built-in `store.count()` or `store.open_cursor().limit(n)` in the stream API.
Full table scan is the only option. For large tables this is fine; for production
pagination, use `open_cursor().with_range(KeyRange::lower_bound(last_id))`.

---

## Recommended Migration Strategy for Phase H

### H2: `silent-storage` with Dexie v2 zero-loss migration

**Read path (Rust `silent-storage`):**

```rust
// Open without version — reads DB at IDB version 20 (Dexie v2).
let db = Database::open("SilentNotetaker").build()?.await?;
```

**Migration pre-flight check:**

```rust
// Verify the DB is at the expected IDB version before migrating.
assert_eq!(db.version(), 20.0, "Expected Dexie v2 DB (IDB version 20)");
```

**Screenshot Blob resolution (separate async pass):**

```javascript
// In JS bridge (capture.js or migration helper):
async function resolveBlobScreenshots(db) {
    const rows = await db.screenshots.toArray();
    for (const row of rows) {
        if (row.image instanceof Blob) {
            const ab = await row.image.arrayBuffer();
            row.image = new Uint8Array(ab);
            await db.screenshots.put(row);
        }
    }
}
```

Run this pass before handing the DB to Rust, or implement it in the Rust migration
via a JS helper exported from `silent-web`.

**Migration phases:**

1. Check IDB version (assert 20 = Dexie v2).
2. Export backup (JSON export + blob bytes).
3. Run Blob → `Uint8Array` normalisation pass.
4. Validate readback (compare counts + spot-check content).
5. Mark migration complete in `localStorage` (key: `silentNotetaker_migrated_v3`).

**Schema version for the Rust-owned DB:**

Rust will own the IDB schema going forward. Choose a fresh Dexie-independent version
scheme: start the Rust DB at IDB version 100 (= a number that will never conflict
with Dexie's ×10 scheme even if someone upgrades Dexie to v9 someday). Use a
monotonic integer, not Dexie at all.

---

## Files Created

```
/Users/mike/dev/snt-spikes/a4-storage/
  Cargo.toml                    # spike crate manifest
  src/lib.rs                    # Rust WASM library (IndexedDB reader)
  src/main.rs                   # placeholder binary for cargo check --all-targets
  fixture.html                  # browser fixture: populates DB + calls Rust readback
  serve.py                      # minimal HTTP server for fixture page
  pkg/                          # wasm-pack --target web output (ES module)
  pkg-no-modules/               # wasm-pack --target no-modules output (used by fixture)
/Users/mike/dev/silent-notetaker/docs/research/spike-storage.md
```

---

## Validation Evidence

```
cargo fmt --all --check                              → clean
cargo test --all-targets                             → 4 passed; 0 failed
cargo clippy --all-targets -- -D warnings            → clean
cargo clippy --target wasm32-unknown-unknown --lib -- -D warnings → clean
wasm-pack build --target no-modules                  → WASM built

Browser readback (Chrome, localhost:8766/fixture.html):
  IDB raw version: 20  (Dexie 2 × 10 confirmed, independently via raw IDBFactory)
  meetings=2, chunks=4, notes=4, screenshots=3
  screenshot encodings exercised: base64 (String), blob (Blob), bytes (Uint8Array)
  image[0] (base64): 1039 bytes match
  image[1] (blob):   67 bytes match   ← migration-critical, two-phase resolved
  image[2] (bytes):  69 bytes match
  totalBlobBytes=1175
  diff: EMPTY — PASS

IDB transaction trap (raw IDBFactory, language-independent):
  TRAP CONFIRMED: reusing tx after a non-IDB await → TransactionInactiveError
  TWO-PHASE CONFIRMED: Blob resolved after tx closed → 67 bytes
```

---

## Migration-critical case: PROVEN (no residual NEEDS-BROWSER-TEST for Blob)

The Blob path that the first spike left as `NEEDS-BROWSER-TEST` is now proven:

- A real JS `Blob` (the migration-critical layout) is read back byte-exact via the
  two-phase async resolution pass.
- The IDB transaction-auto-close trap that makes a naive inline read fail is
  confirmed empirically, and the two-phase design defeats it.
- All three real-world `image` encodings (base64 string / Blob / Uint8Array) pass
  with an EMPTY diff in the same run.

## Remaining Tasks (for H2 productionisation, not blocking the spike)

These are productionisation steps, not unproven unknowns:

- **H2.1**: Run migration against a real END-USER captured DB (a DB produced by the
  shipping app, not a synthetic fixture) to confirm the base64-string rows match a
  live capture exactly. The encoding/byte logic is proven; this is a field-data
  smoke test.
- **H2.2**: Stress-test with a large meeting DB (50+ meetings, 1000+ chunks) — the
  two-phase reader holds all Blob handles in memory between phases; verify memory
  is acceptable, or chunk the resolution pass.
- **H2.3**: Wire the export-backup gate before any normalisation write.
- **H2.4**: Confirm `IDB raw version: 20` and the two-phase reader on Firefox and
  Safari (Safari has historically had IndexedDB quirks, and the Firefox/Safari
  CPU-tier row is a PRD acceptance gate).
