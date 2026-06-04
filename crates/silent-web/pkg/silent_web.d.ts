/* tslint:disable */
/* eslint-disable */

/**
 * Browser-facing diarization surface: TitaNet embedder + SpeakerTracker.
 *
 * # Lifecycle (mirrors nemotron-engine.js)
 *
 * 1. `WasmDiarization.create(onnx_bytes, mel_fb_bytes, dist_base_url)` — loads
 *    the ort-web runtime and builds the ONNX session. Async.
 * 2. `identify(samples)` — embed + track, returns a JSON-serialized
 *    `DiarizationEvent::SpeakerAssigned` (or `null` for the too-short branch).
 * 3. `reuse_last_speaker()` — the too-short segment branch; returns a JSON
 *    `DiarizationEvent::SpeakerAssigned` or `null` if no prior speaker.
 * 4. `evaluate_rename(from_id, value)` — returns a JSON `RenameOutcome`
 *    (`{ "Rename": ... }` or `{ "Merge": ... }`). The UI owns the confirm
 *    dialog; on yes it calls `confirm_merge`.
 * 5. `confirm_merge(from_id, to_id)` — applies the merge; returns a JSON
 *    `DiarizationEvent::MergeApplied`.
 * 6. `rename(id, name)` — plain rename; returns a JSON
 *    `DiarizationEvent::SpeakerRenamed`.
 * 7. `global_recluster(threshold)` — stop-time recluster; returns a JSON
 *    `DiarizationEvent::Reclustered`.
 * 8. `speakers()` — snapshot of the current speaker list as a JSON array of
 *    `SpeakerDescriptor`. Used to rebuild the speakers bar.
 */
export class WasmDiarization {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Apply a merge (the user confirmed the merge-by-rename prompt, or an
     * explicit merge was requested). Folds `from_id` into `to_id`.
     *
     * Returns a JSON `{ "from_id": "S2", "to_id": "S1" }` on success, or
     * `null` if the merge was a no-op (self-merge or unknown id).
     *
     * # Errors
     *
     * Returns a `JsError` only on JSON serialization failure.
     */
    confirm_merge(from_id: string, to_id: string): any;
    /**
     * Create the diarization surface, loading ort-web from the same CDN origin
     * the app already fetches it from (currently `cdn.pyke.io`, in the CSP).
     *
     * - `onnx_bytes`: the TitaNet-small ONNX model bytes.
     * - `mel_fb_json`: the 80×257 slaney mel filterbank, as UTF-8 JSON.
     *
     * # Errors
     *
     * Returns a `JsError` if the ort-web runtime, ONNX session, or mel
     * filterbank cannot be initialised.
     */
    static create(onnx_bytes: Uint8Array, mel_fb_json: Uint8Array): Promise<WasmDiarization>;
    /**
     * Create the diarization surface, loading ort-web from a same-origin
     * vendored base URL (e.g. `"./vendor/"`). Preferred when
     * `crossOriginIsolated === true` and the runtime is vendored same-origin
     * (B3 recommendation).
     *
     * # Errors
     *
     * Returns a `JsError` if the vendored ort-web runtime, ONNX session, or
     * mel filterbank cannot be initialised.
     */
    static create_with_dist(onnx_bytes: Uint8Array, mel_fb_json: Uint8Array, dist_base_url: string): Promise<WasmDiarization>;
    /**
     * Evaluate whether a committed rename is really a merge-by-rename.
     *
     * Returns a JSON `RenameOutcome`:
     * - `{ "tag": "merge", "payload": { "from": "S2", "target": "S1" } }` if
     *   the value matches another speaker's id or name (the UI should confirm).
     * - `{ "tag": "rename", "payload": { "id": "S1", "name": "Alice" } }` for
     *   a plain rename.
     *
     * The UI owns the `confirm()` dialog; on yes it calls `confirm_merge`,
     * on no it calls `rename` directly.
     *
     * # Errors
     *
     * Returns a `JsError` only on JSON serialization failure.
     */
    evaluate_rename(from_id: string, value: string): any;
    /**
     * Run the stop-time global recluster (DIARIZATION.md §2, Appendix A row 15).
     *
     * `threshold` is the cosine similarity above which two clusters merge.
     * Pass `NaN` to use the configured default (0.65).
     *
     * Returns a JSON object:
     * ```json
     * {
     *   "relabel": [{ "old_id": "S5", "new_id": "S2" }, ...],
     *   "speakers": [{ "id": "S1", "name": "Alice", "color": "#00d4aa", "count": 7 }, ...]
     * }
     * ```
     *
     * An empty `relabel` array means no merges were needed.
     *
     * # Errors
     *
     * Returns a `JsError` only on JSON serialization failure.
     */
    global_recluster(threshold: number): any;
    /**
     * Embed + track a segment of 16 kHz mono PCM. This is the hot path called
     * on every utterance boundary.
     *
     * - If `samples.length < min_samples` (default 16 000), calls
     *   `reuse_last_speaker` instead (the JS `minSamples` branch).
     * - On embedder failure, returns `null` (honest degradation — no fake labels).
     *
     * Returns a JSON-serialized object:
     * ```json
     * { "id": "S1", "name": "", "color": "#00d4aa", "is_new": true }
     * ```
     * or `null`.
     *
     * # Errors
     *
     * Returns a `JsError` only on JSON serialization failure (should not occur
     * for these well-typed structs).
     */
    identify(samples: Float32Array): Promise<any>;
    /**
     * Apply a plain rename to a speaker. No return value needed (the UI
     * updates the DOM itself; this keeps the tracker in sync).
     */
    rename(speaker_id: string, new_name: string): void;
    /**
     * Reset the speaker tracker for a new meeting while keeping the loaded
     * ONNX session alive (mirrors the `sharedSpeakerEmbedder` model-survive-
     * meeting-reset semantics — the model stays loaded, the cluster state is
     * cleared). Called from the JS `DiarizationEngine.reset()` method on new
     * meeting.
     */
    reset_tracker(): void;
    /**
     * Return the last-assigned speaker without running the embedder. Called
     * when the segment is too short for a confident embedding (the JS
     * `lastSpeakerId` branch).
     *
     * Returns a JSON `{ "id": "S1", "name": "", "color": "#00d4aa", "is_new": false }`
     * or `null` if there is no prior speaker.
     *
     * # Errors
     *
     * Returns a `JsError` only on JSON serialization failure.
     */
    reuse_last_speaker(): any;
    /**
     * Current snapshot of all speaker clusters, as a JSON array of
     * `SpeakerDescriptor`. Used to rebuild the speakers bar (e.g. after
     * `global_recluster`).
     *
     * # Errors
     *
     * Returns a `JsError` only on JSON serialization failure.
     */
    speakers(): any;
}

/**
 * Browser-facing TitaNet-small speaker embedder.
 *
 * Feed 16 kHz mono f32 audio via [`WasmTitaNetEmbedder::embed`]; receive a
 * 192-d L2-normalized embedding (a flat `Float32Array` in JS).
 */
export class WasmTitaNetEmbedder {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Create an embedder, loading the ort-web runtime from the default CDN.
     *
     * - `onnx_bytes`: bytes of `titanet.onnx` (fetched via the registry pin).
     * - `mel_fb_json`: UTF-8 JSON bytes of `mel_fb.json` (the 80×257 slaney
     *   matrix), also fetched via the registry pin.
     *
     * # Errors
     *
     * Returns a `JsError` if the ort-web runtime, the ONNX session, or the mel
     * filterbank cannot be initialised.
     */
    static create(onnx_bytes: Uint8Array, mel_fb_json: Uint8Array): Promise<WasmTitaNetEmbedder>;
    /**
     * Create an embedder, loading the ort-web runtime from a same-origin
     * vendored base URL (e.g. `"./vendor/"`). The path that keeps
     * `crossOriginIsolated === true` (B3 vendoring).
     *
     * # Errors
     *
     * Returns a `JsError` if the vendored ort-web runtime, the ONNX session, or
     * the mel filterbank cannot be initialised.
     */
    static create_with_dist(onnx_bytes: Uint8Array, mel_fb_json: Uint8Array, dist_base_url: string): Promise<WasmTitaNetEmbedder>;
    /**
     * Compute the 192-d L2-normalized speaker embedding for `samples`
     * (16 kHz mono f32). Returns a `Float32Array` of length 192.
     *
     * # Errors
     *
     * Returns a `JsError` if the mel front-end or ONNX inference fails.
     */
    embed(samples: Float32Array): Promise<Float32Array>;
}

/**
 * Cosine similarity helper exposed to JS for validation (matches the spike's
 * browser-leg cosine check).
 */
export function cosine_sim(a: Float32Array, b: Float32Array): number;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmdiarization_free: (a: number, b: number) => void;
    readonly wasmdiarization_confirm_merge: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly wasmdiarization_create: (a: number, b: number, c: number, d: number) => any;
    readonly wasmdiarization_create_with_dist: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly wasmdiarization_evaluate_rename: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly wasmdiarization_global_recluster: (a: number, b: number) => [number, number, number];
    readonly wasmdiarization_identify: (a: number, b: number, c: number) => any;
    readonly wasmdiarization_rename: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly wasmdiarization_reset_tracker: (a: number) => void;
    readonly wasmdiarization_reuse_last_speaker: (a: number) => [number, number, number];
    readonly wasmdiarization_speakers: (a: number) => [number, number, number];
    readonly __wbg_wasmtitanetembedder_free: (a: number, b: number) => void;
    readonly cosine_sim: (a: number, b: number, c: number, d: number) => number;
    readonly wasmtitanetembedder_create: (a: number, b: number, c: number, d: number) => any;
    readonly wasmtitanetembedder_create_with_dist: (a: number, b: number, c: number, d: number, e: number, f: number) => any;
    readonly wasmtitanetembedder_embed: (a: number, b: number, c: number) => any;
    readonly wasm_bindgen__convert__closures_____invoke__h688f6568f313d5a2: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h17463b5148d2d0d6: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h688f6568f313d5a2_2: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h2d0bf8e6805a65b7: (a: number, b: number, c: any, d: any) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
