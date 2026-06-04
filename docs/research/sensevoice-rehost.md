# SenseVoice First-Party Re-Hosting (Task A5)

**Status: PREPARED — awaiting USER GATE (Mike runs the Hugging Face upload).**

All 8 artifacts are rebuilt from public upstream, hashed, and staged in
`/tmp/sensevoice-rehost/` with a ready-to-run `upload.sh`. The only remaining
step is the upload itself, which is Mike's gate.

---

## ⚠ PRODUCTION INCIDENT (surface to Mike)

**The shipping app's SenseVoice and Dual modes are currently broken on fresh
loads.**

The app loads the SenseVoice engine from a hard-coded k2-fsa Hugging Face
**Space** (`index.html` line 6114):

```
https://huggingface.co/spaces/k2-fsa/web-assembly-vad-asr-sherpa-onnx-zh-en-ja-ko-cantonese-sense-voice/resolve/main
```

As of 2026-06-04 every path on that Space returns **HTTP 401 Unauthorized**
("Invalid username or password") — including `/resolve/main/<file>`,
`/raw/main/<file>`, and an unauthenticated `git clone`. The Space is private,
gated, or deleted. Any user selecting **SenseVoice only** or **Dual (Moonshine
+ SenseVoice)** on a cold cache hits a load failure.

Impact maps to Appendix A row 11 (Dual mode + SenseVoice solo) and the model
picker (row 7). The first-party re-host below is also the fix for this
incident, not just a privacy/pinning improvement.

---

## Resolution: rebuilt from public upstream (better provenance)

Rather than copy the dead Space, the artifacts were rebuilt from the **public**
sherpa-onnx GitHub release. The k2-fsa Space was only ever a hosted copy of this
same prebuilt bundle, so this is a cleaner provenance chain.

### Provenance chain

| Layer | Source |
|---|---|
| WASM bundle | `k2-fsa/sherpa-onnx` GitHub **Release v1.13.2** (2026-05-13) |
| Release asset | `sherpa-onnx-wasm-simd-1.13.2-vad-asr-zh_en_ja_ko_cantonese-sense_voice_small.tar.bz2` |
| Asset sha256 | `ec0a4b6f3ad985f4091df015b74035862023a3bed45714fee4ea5d0600b0285b` |
| Upstream model | [FunAudioLLM/SenseVoiceSmall](https://huggingface.co/FunAudioLLM/SenseVoiceSmall) (ONNX export + WASM packaging by sherpa-onnx) |

Download + repack reproduction (what the agent ran):

```bash
gh release download v1.13.2 --repo k2-fsa/sherpa-onnx \
  --pattern "sherpa-onnx-wasm-simd-1.13.2-vad-asr-zh_en_ja_ko_cantonese-sense_voice_small.tar.bz2"
tar -xjf sherpa-onnx-wasm-simd-1.13.2-vad-asr-zh_en_ja_ko_cantonese-sense_voice_small.tar.bz2
```

### Why v1.13.2 (version selection)

The app's `SenseVoiceEngine` (`index.html` 6116–6262) uses the **legacy
direct-constructor** sherpa-onnx WASM API, not the newer `createOfflineRecognizer`
factory. The exact call sites and their match in the v1.13.2 bundle:

| App call (index.html) | v1.13.2 bundle symbol | Match |
|---|---|---|
| `new OfflineRecognizer(config, this.module)` | `class OfflineRecognizer { constructor(configObj, Module) }` in `sherpa-onnx-asr.js` | ✅ |
| `new Vad(vadConfig, this.module)` | `class Vad { constructor(configObj, Module) }` in `sherpa-onnx-vad.js` | ✅ |
| `new CircularBuffer(30*16000, this.module)` | `class CircularBuffer { constructor(capacity, Module) }` | ✅ |
| `recognizer.createStream / decode / getResult`, `stream.acceptWaveform / free` | all present in `sherpa-onnx-asr.js` | ✅ |
| `vad.acceptWaveform / isEmpty / front / pop`, `vad.config.sileroVad.windowSize` | all present in `sherpa-onnx-vad.js` | ✅ |
| `buffer.push / size / head / get / pop` | all present | ✅ |
| config `modelConfig.senseVoice.{model,useInverseTextNormalization}` | both keys present in `sherpa-onnx-asr.js` | ✅ |

**Version risk:** sherpa-onnx keeps this constructor-style API stable across
the 1.10–1.13 line, so v1.13.2 is the latest release that still matches the
app's call sites with zero code changes. If a future sherpa-onnx release
removes the direct constructors, the app's loader (not just the registry) would
need updating — pin to this revision until that's deliberately addressed.

---

## Artifact set (8 files)

Extracted to `/tmp/sensevoice-rehost/`. The Emscripten bundle physically ships
**5 files**; the **3 model files are packed inside** `…vad-asr.data`. The app
only fetches those 5 over HTTP (≈ **241.5 MiB ≈ the "~253MB" the UI quotes**);
the 3 models load from the Emscripten virtual FS, never as separate requests.
The 3 models are unpacked here too, for standalone registry hashing and
reproducibility.

| File | Size (bytes) | sha256 | Fetched by app over HTTP? |
|---|---|---|---|
| `sherpa-onnx-asr.js` | 53,867 | `d51ae8e8b756ee5e53423ffada0c9702973f154f561aca7984fe0b12f4060178` | yes |
| `sherpa-onnx-vad.js` | 7,772 | `893f01168d529add8318c0a6055cf725e788585fda9b81722564a8c3c3f60e34` | yes |
| `sherpa-onnx-wasm-main-vad-asr.js` | 116,698 | `f4b0e1ec27706d971f31e7749f2f9be15c64a2604f864c08b42f2c0d5f2a8fd9` | yes |
| `sherpa-onnx-wasm-main-vad-asr.wasm` | 12,898,602 | `a1f3fb15701fad8c556af45d785ddd17dcfe8e25272aa1a02ba0369c9f2ce828` | yes (via `locateFile`) |
| `sherpa-onnx-wasm-main-vad-asr.data` | 240,193,589 | `4c063aa4af215b02b6c127f3b7be8ae8405ff1285a18117e746f4abe53e5b3be` | yes (via `locateFile`) |
| `sense-voice.onnx` | 239,233,841 | `c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51` | no — packed in `.data` |
| `silero_vad.onnx` | 643,854 | `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6` | no — packed in `.data` |
| `tokens.txt` | 315,894 | `f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc` | no — packed in `.data` |

Machine-readable copy: `/tmp/sensevoice-rehost/manifest.csv`.

The 3 model byte-ranges within `.data` (from the file_packager metadata in the
`.js` loader; used to unpack them):

```
/sense-voice.onnx  start=0          end=239233841
/silero_vad.onnx   start=239233841  end=239877695
/tokens.txt        start=239877695  end=240193589   (remote_package_size=240193589)
```

Validation of extracted models:
- `sense-voice.onnx` — valid ONNX (contains `onnx.quantize`, `onnx::Gather_*` — INT8 quantized).
- `silero_vad.onnx` — ONNX binary.
- `tokens.txt` — UTF-8, 25,055 tokens, SentencePiece-style (`<unk> 0`, `<s> 1`, `</s> 2`, `▁the 3` …).

---

## Loader reference (index.html 6110–6262)

```javascript
const SPACE_BASE = 'https://huggingface.co/spaces/k2-fsa/web-assembly-vad-asr-sherpa-onnx-zh-en-ja-ko-cantonese-sense-voice/resolve/main';

// JS API scripts:
await this._loadScript(`${SPACE_BASE}/sherpa-onnx-asr.js`);
await this._loadScript(`${SPACE_BASE}/sherpa-onnx-vad.js`);

// Emscripten loader; locateFile redirects ONLY .wasm and .data:
locateFile: (path) => (path.endsWith('.wasm') || path.endsWith('.data'))
  ? `${SPACE_BASE}/${path}` : path;
script.src = `${SPACE_BASE}/sherpa-onnx-wasm-main-vad-asr.js`;

// Models referenced from the Emscripten VFS (NOT fetched separately):
//   recognizer: tokens './tokens.txt', senseVoice.model './sense-voice.onnx'
//   vad:        sileroVad.model './silero_vad.onnx'
```

---

## USER GATE — Mike runs the upload (agents must NOT)

Per the orchestration spec gate table:
`HF upload: SenseVoice first-party repo | A5 → D1 registry pin | pending (A5 preps everything)`.

### Step 1 — review what's staged
```bash
ls -lh /tmp/sensevoice-rehost/
cat /tmp/sensevoice-rehost/manifest.csv
```

### Step 2 — log in to Hugging Face (write token)
```bash
pip install -U "huggingface_hub[cli]"
huggingface-cli login
```

### Step 3 — run the upload script
Targets a first-party **model** repo, mirroring `FluffyBunnies/titanet-small-onnx`:
```bash
/tmp/sensevoice-rehost/upload.sh FluffyBunnies/sensevoice-sherpa-onnx
```
The script: verifies local hashes against `manifest.csv`, creates the repo,
uploads the 5 harness files + the 3 unpacked models + `manifest.csv` +
`README.md`, then prints the next pin step.

### Step 4 — pin the revision
```bash
# Copy the upload commit SHA from:
#   https://huggingface.co/FluffyBunnies/sensevoice-sherpa-onnx/commits/main
```

### Step 5 — point the app at the pinned first-party repo
Edit `index.html` line 6114:
```javascript
// OLD (dead k2-fsa Space):
// const SPACE_BASE = 'https://huggingface.co/spaces/k2-fsa/.../resolve/main';

// NEW (first-party, pinned to commit SHA):
const SPACE_BASE = 'https://huggingface.co/FluffyBunnies/sensevoice-sherpa-onnx/resolve/<COMMIT_SHA>';
```
(Phase-1 work moves this URL into the registry; for the production-incident hotfix
the inline edit is enough to un-break SenseVoice/Dual immediately.)

### Step 6 — hand off to registry (Task D1)
Give D1 the repo + commit SHA. The registry entry (below) already has the
per-file sha256 values; D1 only needs to fill `repo` and `revision`.

---

## Registry entry (for Task D1; sha256 already verified)

```toml
[[model]]
id = "asr.sensevoice.sherpa_small"
task = "asr"
provider = "huggingface"
repo = "FluffyBunnies/sensevoice-sherpa-onnx"   # set after upload
revision = "<COMMIT_SHA>"                         # set after upload (no `main`)
host = "js-sherpa"                                # sherpa-onnx Emscripten host
execution_provider = "cpu"
precision = ["int8"]
memory_budget_mb = 350
cache = "cache-api"
license = "apache-2.0"                            # confirm in Task A2
license_verified = false                          # A2 flips this
network_origins = ["https://huggingface.co", "https://cdn-lfs.huggingface.co"]

  # The 5 files the app fetches over HTTP:
  [[model.files]]
  path = "sherpa-onnx-asr.js"
  size = 53867
  sha256 = "d51ae8e8b756ee5e53423ffada0c9702973f154f561aca7984fe0b12f4060178"
  [[model.files]]
  path = "sherpa-onnx-vad.js"
  size = 7772
  sha256 = "893f01168d529add8318c0a6055cf725e788585fda9b81722564a8c3c3f60e34"
  [[model.files]]
  path = "sherpa-onnx-wasm-main-vad-asr.js"
  size = 116698
  sha256 = "f4b0e1ec27706d971f31e7749f2f9be15c64a2604f864c08b42f2c0d5f2a8fd9"
  [[model.files]]
  path = "sherpa-onnx-wasm-main-vad-asr.wasm"
  size = 12898602
  sha256 = "a1f3fb15701fad8c556af45d785ddd17dcfe8e25272aa1a02ba0369c9f2ce828"
  [[model.files]]
  path = "sherpa-onnx-wasm-main-vad-asr.data"
  size = 240193589
  sha256 = "4c063aa4af215b02b6c127f3b7be8ae8405ff1285a18117e746f4abe53e5b3be"

  # The 3 models packed inside .data (uploaded standalone for provenance):
  [[model.files]]
  path = "sense-voice.onnx"
  size = 239233841
  sha256 = "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51"
  [[model.files]]
  path = "silero_vad.onnx"
  size = 643854
  sha256 = "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"
  [[model.files]]
  path = "tokens.txt"
  size = 315894
  sha256 = "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc"

  [model.device_tiers]
  wasm_only  = { default_for_tier = true }
  webgpu_low = { default_for_tier = false }
```

---

## Validation checklist

Agent-side (done):
- [x] Public upstream source identified (sherpa-onnx Release v1.13.2 SenseVoice vad-asr asset).
- [x] All 8 artifacts staged in `/tmp/sensevoice-rehost/`.
- [x] sha256 + size recorded for every file (`manifest.csv`).
- [x] API compatibility verified against actual index.html call sites.
- [x] Extracted models validated (ONNX magic, token vocab).
- [x] `upload.sh` + `README.md` + registry entry written.

User-side (USER GATE — Mike):
- [ ] `upload.sh` run; first-party repo populated.
- [ ] Revision pinned (commit SHA captured).
- [ ] `index.html` `SPACE_BASE` repointed to the pinned first-party URL.
- [ ] Fresh-load browser test: 5 harness files fetch 200 OK; SenseVoice + Dual transcribe.
- [ ] Hand commit SHA to Task D1 for registry `repo`/`revision`.

---

## Files staged

```
/tmp/sensevoice-rehost/
  sherpa-onnx-asr.js
  sherpa-onnx-vad.js
  sherpa-onnx-wasm-main-vad-asr.js
  sherpa-onnx-wasm-main-vad-asr.wasm
  sherpa-onnx-wasm-main-vad-asr.data
  sense-voice.onnx          # unpacked from .data
  silero_vad.onnx           # unpacked from .data
  tokens.txt                # unpacked from .data
  manifest.csv              # path,size,sha256,role,fetched_over_http_by_app
  README.md                 # HF repo card with provenance
  upload.sh                 # USER-GATE upload script (chmod +x)
```

## References
- index.html `SenseVoiceEngine` — lines 6110–6262
- PRD R3 (swappable models), R4 (registry), Appendix C ("SenseVoice kept; artifacts re-hosted first-party")
- TitaNet pattern: `FluffyBunnies/titanet-small-onnx`
- Upstream: `k2-fsa/sherpa-onnx` Release v1.13.2; `FunAudioLLM/SenseVoiceSmall`
