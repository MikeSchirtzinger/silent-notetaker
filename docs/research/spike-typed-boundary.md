# Spike: Typed Boundary — ts-rs vs tsify for `#[non_exhaustive]` Event Enums

**Task**: A3 (a3-spike-boundary)  
**Spike dir**: `/Users/mike/dev/snt-spikes/a3-typed-boundary/`  
**Status**: COMPLETED  

## Verdict

**Use ts-rs.** It wins on every axis that matters for this project.

## What was evaluated

The spike modelled the exact PRD `EngineEvent` enum plus `UiCommand` and `SessionEvent`
using BOTH `ts-rs 10.1` and `tsify-next 0.5` on the same Rust types simultaneously.
Generated outputs were compiled with `tsc --noEmit --strict` against representative
UI-style consumer code including the `#[non_exhaustive]` discriminated-union escape hatch.

All Rust tests pass. Clippy is clean. TypeScript compilation has zero errors.

---

## Comparison

### Output shape

Both tools produce compatible discriminated-union TypeScript. The `serde(tag = "tag",
content = "payload", rename_all = "snake_case")` layout produces the same discriminant
structure in both:

```typescript
// ts-rs output (EngineEvent excerpt):
export type EngineEvent =
  | { "tag": "load_progress"; "payload": { file: string; loaded: bigint; total: bigint } }
  | { "tag": "ready" }
  | { "tag": "partial"; "payload": { text: string; range: TimeRange } }
  | { "tag": "final";   "payload": { text: string; range: TimeRange } }
  | { "tag": "stats";   "payload": EngineStats }
  | { "tag": "warning"; "payload": { message: string } };

// tsify output (same enum, from wasm-pack .d.ts):
export type EngineEvent =
  | { tag: "load_progress"; payload: { file: string; loaded: number; total: number } }
  | { tag: "ready" }
  | { tag: "partial"; payload: { text: string; range: TimeRange } }
  | { tag: "final";   payload: { text: string; range: TimeRange } }
  | { tag: "stats";   payload: EngineStats }
  | { tag: "warning"; payload: { message: string } };
```

### Critical difference: u64 → bigint (ts-rs) vs number (tsify)

This is the decisive factor.

ts-rs maps Rust `u64` to TypeScript `bigint`. tsify maps it to `number`.

`number` is IEEE 754 double-precision, which loses integer precision above
`Number.MAX_SAFE_INTEGER` (2^53 - 1 = 9,007,199,254,740,991). For this boundary:

- File sizes (encoder.onnx ~881 MB, Voxtral ~2.7 GB): well within safe integer range.
- Meeting IDs, chunk counts: safe in practice.
- `u64::MAX` = 18,446,744,073,709,551,615: 2000x beyond safe integer range.

The tsify `number` mapping is technically wrong for u64. It will not cause bugs for
current values, but it silently allows TypeScript code to lose precision without the
compiler warning. `bigint` at the boundary is correct. If `bigint` is operationally
inconvenient, the right fix is to use `u32` or `f64` in the Rust type definition, not
to accept a lossy TS mapping.

### Secondary difference: null vs undefined for Option<T>

ts-rs maps `Option<T>` to `T | null`. tsify maps it to `T | undefined`.

Both are idiomatic TypeScript, but they behave differently under `exactOptionalPropertyTypes`:

- `null` explicitly signals "absent but present in the serialized form" (serde's default)
- `undefined` in TypeScript conventionally means "not present" (JSON omits it)

Since serde serializes `None` as `null` in JSON (unless `#[serde(skip_serializing_if)]`
is used), the ts-rs `null` mapping is more accurate to the wire format.

### Other differences

| Dimension | ts-rs | tsify |
|---|---|---|
| u64 mapping | `bigint` (exact) | `number` (lossy) |
| Option<T> mapping | `T \| null` | `T \| undefined` |
| Output | One `.ts` file per type | One `.d.ts` in wasm-pack bundle |
| Integration | Separate `cargo test` step | Automatic via wasm-pack |
| Coupling | None — pure codegen | Requires wasm-bindgen on every exported type |
| Build step | `cargo test export_bindings` | `wasm-pack build` |
| Struct field keys | `"start_ms"` (quoted) | `start_ms` (unquoted) |
| File splitting | Yes — importable individually | No — monolithic |
| Doc comments | Preserved in output | Preserved in output |
| Non-exhaustive | Not expressed in output | Not expressed in output |
| Works without wasm-pack | Yes | No |

---

## How `#[non_exhaustive]` maps to TypeScript

Neither ts-rs nor tsify can encode `#[non_exhaustive]` in the TypeScript type system,
and that is the correct answer — TypeScript has no equivalent of Rust's `#[non_exhaustive]`.

The idiomatic TypeScript encoding is a **mandatory wildcard arm** in every `switch` over
the union:

```typescript
// WRONG — will crash if Rust adds a new variant in a minor version:
function handle(event: EngineEvent): void {
    switch (event.tag) {
        case "partial": ...; break;
        case "final":   ...; break;
        default: assertNever(event); // TypeScript says "ok"; runtime says "crash"
    }
}

// CORRECT — the #[non_exhaustive] escape hatch:
function handle(event: EngineEvent): void {
    switch (event.tag) {
        case "partial": ...; break;
        case "final":   ...; break;
        // ... all known variants ...
        default: {
            // `event` is narrowed to `never` here (all known variants handled),
            // but we widen to `{ tag: string }` so a future Rust variant won't crash.
            const _unknown: { tag: string } = event;
            console.warn("[boundary] unknown event variant:", _unknown.tag);
        }
    }
}
```

The pattern is:
1. Handle all known variants explicitly.
2. `default:` arm with `const _: { tag: string } = event` — this is assignable from
   both `never` (when all known variants are handled) and from any future union member
   Rust adds (since all tagged variants are assignable to `{ tag: string }`).
3. Do NOT call `assertNever(event)` in the default arm.

This pattern is demonstrated and verified in the spike's `consumer.ts` including a
simulated future-variant test.

**Enforcement strategy for C1**: Add a lint comment to the generated TS header:

```
// WARNING: EngineEvent is #[non_exhaustive] in Rust.
// Your switch MUST include a default arm that does NOT call assertNever.
// See docs/research/spike-typed-boundary.md for the escape-hatch pattern.
```

ts-rs supports a `#[ts(rename_all)]` custom attribute for header injection. Alternatively,
a post-processing step can append the warning to the generated file.

---

## Build integration into wasm-pack

### ts-rs integration (recommended)

ts-rs generates TypeScript at test time, not build time. The canonical integration:

```toml
# In silent-web/Cargo.toml:
[dev-dependencies]
ts-rs = { version = "10", features = ["serde-compat"] }
```

```bash
# In xtask or Makefile:
cargo test -p silent-web export_bindings
# Writes to silent-web/bindings/*.ts (or a configured output dir)
```

The generated files are committed to the repo (they are the contract). CI checks that
they are fresh:

```bash
cargo test -p silent-web export_bindings
git diff --exit-code crates/silent-web/bindings/
```

A stale bindings dir fails CI, enforcing that the Rust types and TS types stay in sync.

### tsify integration (alternative, not recommended)

tsify generates TypeScript automatically during `wasm-pack build`. No separate step.
The `.d.ts` is part of the wasm-pack output bundle.

The problem: this ties TypeScript type generation to the wasm compile. You cannot
regenerate types without a full wasm-pack build (~56 seconds in the spike). The ts-rs
approach is faster and more composable.

### For `silent-web` specifically

The PRD specifies `silent-web` as the `wasm-bindgen` boundary crate. The recommended
layout:

```
crates/silent-web/
  src/
    lib.rs          # wasm-bindgen exports; does NOT derive TS
  types.rs          # or crates/silent-core/src/events.rs
    # The Rust types derive TS and Tsify
  bindings/         # committed generated TS
    EngineEvent.ts
    UiCommand.ts
    SessionEvent.ts
    ...
```

`silent-core` derives `TS` (no wasm-bindgen dep). `silent-web` depends on `silent-core`
and re-exports through wasm-bindgen. The `#[ts(export)]` attributes live in `silent-core`
where the types are defined, not in `silent-web`.

---

## Blockers for C1

1. **No blockers.** ts-rs 10 compiles against Rust 1.95 (current stable), generates
   correct output, and `tsc --noEmit` passes.

2. **The `serde(transparent)` warning** from ts-rs is cosmetic: ts-rs cannot parse
   `serde(transparent)` and ignores it. The generated TypeScript for `ModelId` is still
   correct (`type ModelId = string`). Workaround: use `#[ts(transparent)]` explicitly
   alongside `#[serde(transparent)]`. This can be added in C1 when `silent-core` is
   created.

3. **The `bigint` cost**: TypeScript call sites that construct `EngineEvent::LoadProgress`
   must use `BigInt` literals (`500n` not `500`). This is a minor ergonomic cost in the
   current `index.html` JS (which would use `Number(loaded)` when receiving the event
   from Rust). Since the current UI reads events from Rust (not constructs them), this
   mostly affects test code. The fix is to type-assert on the receiving side or use
   `Number()` where precision is not a concern.

4. **The current `index.html` is untyped JS**: importing the generated `.ts` files
   requires either (a) a TypeScript compile step on `index.html` (not planned — the UI
   doesn't change) or (b) using the types as documentation only, with runtime validation.
   The PRD's value here is compile-time checking of the Rust types themselves and
   future typed modules (`capture.js`, `transformers-host.js`). These are the files
   that C1/G3/I1 will introduce as TypeScript modules, checked against the generated
   boundary types.

---

## Files created

- `/Users/mike/dev/snt-spikes/a3-typed-boundary/Cargo.toml`
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/src/lib.rs`
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/ts-consumer/consumer.ts`
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/ts-consumer/consumer-tsify.ts`
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/ts-consumer/package.json`
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/ts-consumer/tsconfig.json`
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/bindings/` — ts-rs generated output
- `/Users/mike/dev/snt-spikes/a3-typed-boundary/pkg/` — tsify wasm-pack output

## Validation commands run

```bash
# Rust
cargo fmt --all --check              # PASS
cargo check                          # PASS (1 cosmetic ts-rs warning on serde(transparent))
cargo clippy --all-targets -D warnings  # PASS — 0 clippy warnings
cargo test                           # PASS — 10 tests

# TypeScript
npx tsc --noEmit                     # PASS — 0 errors
# (checks consumer.ts against ts-rs bindings AND consumer-tsify.ts against tsify .d.ts)
```
