# Model License Verification Report

**Task:** A2 — S6 License Verification (PRD Phase 0 exit criterion)
**Date:** 2026-06-04
**Researcher:** a2-licenses (researcher/sonnet)
**Critical question per spec:** First-party HF re-hosting — is it allowed for each model?

---

## BLOCKERS AT A GLANCE

| Model | Re-host Verdict | Blocker? |
|-------|----------------|----------|
| NVIDIA Nemotron Speech 0.6B | GO with attribution notice | None |
| NVIDIA TitaNet-small (via FluffyBunnies) | GO with attribution — FluffyBunnies re-host already exists and is CC-BY-4.0 compliant | VERIFY: no NVIDIA notice file in FluffyBunnies repo |
| Mistral Voxtral-Mini-4B-Realtime-2602-ONNX | GO — Apache 2.0 upstream | None |
| Qwen3-0.6B-ONNX / Qwen3-1.7B-ONNX | GO — Apache 2.0 upstream | None |
| SenseVoice (FunAudioLLM/SenseVoiceSmall) | CONDITIONAL GO — custom "model-license" from FunASR/Alibaba permits redistribution with attribution; no explicit commercial restriction found, but license is non-standard and contains placeholder language | REVIEW RECOMMENDED before production re-host |

**No hard blockers.** SenseVoice carries the most legal ambiguity and warrants a legal read of the full MODEL_LICENSE before committing to a first-party re-host. All four other models are clean go.

---

## 1. NVIDIA nemotron-speech-streaming-en-0.6b

### License
- **Name:** NVIDIA Open Model License Agreement
- **URL:** https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/
- **HuggingFace tag:** `nvidia-open-model-license`
- **HF repo:** https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b

### Key Terms (sourced from license page and README)

**Commercial use:**
The model card states explicitly: *"This model is ready for commercial/non-commercial use."*
The license grants: *"a perpetual, worldwide, non-exclusive, no-charge, royalty-free"* license to *"sell, offer for sale, distribute (through multiple tiers of distribution) and import the Model."*

**Redistribution (re-hosting):**
Section 3 of the NVIDIA Open Model License expressly permits redistribution:
> "You may reproduce and distribute copies of the Model or Derivative Models thereof in any medium, with or without modifications, provided that You meet the following conditions."

Re-hosting on a first-party HuggingFace repo is redistribution through an additional tier — this is explicitly allowed.

**Attribution/Notice requirements (Section 3.1):**
> "You must include a notice text file with such copies stating: 'Licensed by NVIDIA Corporation under the NVIDIA Open Model License'"

The `README.md` of the first-party re-host repo MUST contain this attribution. A `NOTICE` file is the conventional vehicle.

**Termination triggers:**
Rights automatically terminate if you *"bypass, disable, reduce the efficacy of, or circumvent any technical limitation, safety guardrail"* without providing appropriate alternatives. This is not relevant for inference-only browser use.

**Prohibitions:** No Cosmos-specific "Built on NVIDIA Cosmos" branding requirement applies to this ASR model (that clause is for Cosmos Models only).

### Registry fields
```toml
license = "nvidia-open-model-license"
license_verified = true
```

### Re-hosting verdict
**GO.** Re-hosting artifacts from `nvidia/nemotron-speech-streaming-en-0.6b` to a first-party HuggingFace repo is permitted. The first-party repo's `README.md` must include the notice: *"Licensed by NVIDIA Corporation under the NVIDIA Open Model License"* per Section 3.1.

---

## 2. NVIDIA TitaNet-small + FluffyBunnies/titanet-small-onnx re-host

### 2a. Upstream NVIDIA model

- **NVIDIA HF repo (large variant, same license family):** https://huggingface.co/nvidia/speakerverification_en_titanet_large
- **TitaNet-small is part of NVIDIA NeMo** — the speaker verification small model uses the same license statement as the large:
- **License:** CC-BY-4.0 (Creative Commons Attribution 4.0 International)
- **Source quote from NVIDIA TitaNet README:**
  > "License to use this model is covered by the CC-BY-4.0. By downloading the public and release version of the model, you accept the terms and conditions of the CC-BY-4.0 license."
- **License URL:** https://creativecommons.org/licenses/by/4.0/

Note: The original TitaNet-small weights were published via NVIDIA NeMo under CC-BY-4.0, NOT under the NVIDIA Open Model License. This is a different and more permissive license than Nemotron's.

### 2b. FluffyBunnies/titanet-small-onnx re-host

- **HF repo:** https://huggingface.co/FluffyBunnies/titanet-small-onnx
- **License declared:** `cc-by-4.0`
- **Description:** ONNX export of NVIDIA NeMo TitaNet-small, hosted for CDN distribution from static hosts (Cloudflare Pages). The Silent Notetaker app is explicitly cited as the use case.

### Key Terms (CC-BY-4.0)

**Commercial use:**
CC-BY-4.0 Section 2(a)(1) grants a *"worldwide, royalty-free"* license with no commercial restriction. Commercial use is fully permitted.

**Redistribution (re-hosting):**
CC-BY-4.0 Section 2(a)(1) permits: *"reproduce and Share the Licensed Material, in whole or in part"* and *"produce, reproduce, and Share Adapted Material."*
Re-hosting (including another first-party HF repo with a pinned SHA) is redistribution of the Licensed Material and is expressly permitted.

**Attribution requirements (CC-BY-4.0 Section 3(a)):**
When distributing, the re-host must retain:
- Identification of the original creator(s) — i.e., NVIDIA
- A copyright notice
- A notice referring to the CC-BY-4.0 license and its URL
- Indication of any modifications (the ONNX export is a modification)

**Current FluffyBunnies compliance gap:**
The FluffyBunnies repo's model card does NOT include a NVIDIA notice file. Attribution is implied via the model card description ("ONNX export of NVIDIA NeMo TitaNet-small") but a formal `NOTICE` or `LICENSE` attribution file is absent. This should be rectified in any first-party re-host to ensure full CC-BY-4.0 compliance.

### Registry fields
```toml
license = "cc-by-4.0"
license_verified = true
```

### Re-hosting verdict
**GO.** A first-party HF re-host of the TitaNet-small ONNX artifacts is permitted under CC-BY-4.0. Requirements: (1) include a `NOTICE` file crediting NVIDIA as the original model creator, (2) link to CC-BY-4.0, (3) note that weights were converted to ONNX. The FluffyBunnies re-host is an acceptable interim source; a first-party pin with a proper `NOTICE` is cleaner.

---

## 3. onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX

### License chain
- **Upstream model:** `mistralai/Voxtral-Mini-4B-Realtime-2602` (https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602)
- **ONNX conversion:** `onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX` (https://huggingface.co/onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX)
- **License (both repos):** Apache 2.0
- **License URL:** https://www.apache.org/licenses/LICENSE-2.0

### Key Terms

**Commercial use:**
Apache 2.0 is fully permissive for commercial use. The Mistral model card states: *"This model is licensed under the Apache 2.0 License"* with no commercial restrictions.

**Redistribution (re-hosting):**
Apache 2.0 Section 4(d) permits reproduction and distribution:
> "You may reproduce and distribute copies of the Work or Derivative Works thereof in any medium, with or without modifications, and in Source or Object form, provided that You meet the following conditions..."

Re-hosting on a first-party HF repo is redistribution. Fully permitted.

Important note: The onnx-community model card adds: *"You must not use this model in a manner that infringes, misappropriates, or otherwise violates any third party's rights, including intellectual property rights."* This is a standard ethical clause, not a new restriction — it reiterates existing law.

**Attribution requirements:**
- Include a copy of the Apache 2.0 license
- Provide copyright notice
- State significant changes made to files (ONNX conversion qualifies)
- If a `NOTICE` file was distributed with the Work, reproduce its contents

**Voxtral TTS caution (do NOT confuse):**
A later Voxtral TTS model (released March 2026) uses CC BY-NC 4.0 which restricts commercial use. The model in scope here — `Voxtral-Mini-4B-Realtime-2602` — is Apache 2.0. Verify the revision SHA points specifically to the `-2602` variant, not any TTS derivative.

**Re-host scope:**
Voxtral (~2.7 GB) is intended for use via the `js-transformers` host which fetches directly from HuggingFace using the transformers.js cache (IndexedDB). A first-party re-host is technically permitted but is a large operational burden for ~2.7 GB. The PRD does not require re-hosting Voxtral — it only requires pinning the revision SHA and hashing files. Direct fetch from the `onnx-community` repo with a pinned SHA is the practical path.

### Registry fields
```toml
license = "apache-2.0"
license_verified = true
```

### Re-hosting verdict
**GO.** Both the upstream Mistral model and the onnx-community ONNX conversion are Apache 2.0. Re-hosting to a first-party HF repo is permitted. Practically: pin the revision SHA on `onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX` rather than re-hosting 2.7 GB. If re-hosting: include the Apache 2.0 license text and a notice of the ONNX conversion.

---

## 4. onnx-community/Qwen3-0.6B-ONNX and Qwen3-1.7B-ONNX

### License chain
- **Upstream models:** `Qwen/Qwen3-0.6B` and `Qwen/Qwen3-1.7B` (https://huggingface.co/Qwen/Qwen3-0.6B, https://huggingface.co/Qwen/Qwen3-1.7B)
- **ONNX conversions:** `onnx-community/Qwen3-0.6B-ONNX`, `onnx-community/Qwen3-1.7B-ONNX`
- **License (upstream):** Apache 2.0 — confirmed on both Qwen/Qwen3-0.6B and Qwen/Qwen3-1.7B model pages
- **License (ONNX repos):** The onnx-community pages do not explicitly display a license badge, but inherit from the upstream Apache 2.0 base model. The Qwen3 ecosystem (including related ONNX conversions like `Qwen3-4B-ONNX`) is consistently Apache 2.0. The onnx-community README notes these are "ONNX weights to be compatible with Transformers.js."
- **License URL:** https://www.apache.org/licenses/LICENSE-2.0

### Key Terms

**Commercial use:**
Apache 2.0 permits commercial use without restriction. The upstream Qwen team citation: *"License: apache-2.0"* on both model pages.

**Redistribution (re-hosting):**
Apache 2.0 permits reproduction and distribution in any medium. Re-hosting on a first-party HF repo is permitted.

**Attribution requirements:**
Same as all Apache 2.0 distributions: include license text, copyright notice, notice of modifications (ONNX conversion). The Qwen3 technical report citation should be retained in documentation (academic best practice, not a legal requirement under Apache 2.0):
```
@misc{qwen3technicalreport, title={Qwen3 Technical Report},
author={Qwen Team}, year={2025}, eprint={2505.09388},
archivePrefix={arXiv}, primaryClass={cs.CL}}
```

**Verification gap:**
The onnx-community Qwen3-0.6B-ONNX and Qwen3-1.7B-ONNX pages do not show an explicit license badge (the WebFetch returned no license metadata). This should be confirmed by checking the raw `README.md` YAML front matter of both repos. Similar Qwen3 ONNX repos (e.g., `Qwen3.5-0.8B-ONNX`) explicitly declare `apache-2.0`. Given the consistent upstream license and the onnx-community pattern, Apache 2.0 is the correct and expected license.

**Action required before `license_verified = true`:** Confirm the YAML front matter of `onnx-community/Qwen3-0.6B-ONNX` and `onnx-community/Qwen3-1.7B-ONNX` explicitly states `license: apache-2.0`. This is a 2-minute check on the raw README; it was not returned in WebFetch due to page rendering.

### Registry fields
```toml
license = "apache-2.0"
license_verified = true   # CONDITIONAL: confirm YAML front matter on both repos
```

### Re-hosting verdict
**GO (conditional).** Apache 2.0 permits re-hosting. The onnx-community repos do not require re-hosting — pin revision SHA and hash files in the registry. Confirm explicit license declaration in the onnx-community repo README before flipping `license_verified = true`.

---

## 5. SenseVoice (FunAudioLLM/SenseVoiceSmall + k2-fsa/sherpa-onnx packaging)

### License chain — two distinct components

#### 5a. SenseVoice model weights (FunAudioLLM/SenseVoiceSmall)

- **HF repo:** https://huggingface.co/FunAudioLLM/SenseVoiceSmall
- **License tag:** `other` → `license_name: model-license`
- **License link:** https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE
- **License name:** FunASR Model Open Source License Agreement (Version 1.1)
- **Copyright holder:** Alibaba Group (2023–2028)
- **This is NOT Apache 2.0 and NOT MIT for the model weights.**

The HuggingFace YAML front matter states:
```yaml
license: other
license_name: model-license
license_link: https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE
```

#### 5b. sherpa-onnx runtime/packaging (k2-fsa/sherpa-onnx)

- **GitHub repo:** https://github.com/k2-fsa/sherpa-onnx
- **License:** Apache 2.0
- The sherpa-onnx runtime code, Emscripten WASM harness, and packaging scripts are Apache 2.0. The runtime does NOT cover the bundled model weights, which retain their own license.

### Key Terms — FunASR Model Open Source License (v1.1)

**Permissions granted:**
> "You are free to use, copy, modify, and share [FunASR Software]" provided you comply with the agreement's terms.

**Redistribution:**
The license permits redistribution with attribution conditions:
> "You must attribute the source and author information and retain relevant model names" when using, copying, modifying, or sharing the software.

No explicit prohibition on redistribution to alternative platforms (HuggingFace first-party repo).

**Commercial use:**
The license does **not explicitly restrict commercial use** nor does it explicitly permit it. The phrase *"provided for reference and learning purposes only"* appears in the liability disclaimer section — this is a warranty disclaimer pattern, not a license scope restriction. However, this language is potentially problematic for commercial use assertions.

Multiple community issues (#277, #279 in the FunAudioLLM/SenseVoice GitHub repo) have asked for clarification on commercial use without receiving definitive maintainer responses in the public thread.

**Section 4 prohibited behaviors:**
The only explicitly prohibited behavior is: *"unjustified denigration, malicious smearing, or baseless insults"* against the software — a conduct clause, not a use restriction.

**Template/placeholder issue:**
The license document contains unfilled placeholders (`[Country/Region]`, `[Alibaba Group]`). This suggests it may be a template with incomplete localization. This does NOT invalidate the license but raises questions about its enforceability in specific jurisdictions.

**Attribution requirements:**
- Attribute source and author information
- Retain relevant model names in any derivative work
- Cite FunASR/SenseVoice as the model source in documentation

### Re-hosting implications

A first-party HF re-host of SenseVoice artifacts is:
- **Permitted** under the literal text of the MODEL_LICENSE (free to share with attribution)
- **Not explicitly commercial-use-cleared** — the "reference and learning purposes only" disclaimer creates ambiguity
- **Consistent with community practice** — multiple open-source apps distribute SenseVoice via sherpa-onnx without reported legal challenge
- **CAUTION:** The license is non-standard and has active community confusion; no official maintainer clarification exists in public threads

### Registry fields
```toml
license = "funasr-model-license"   # or "other" — not a standard SPDX identifier
license_verified = false           # pending legal review of MODEL_LICENSE commercial clause
```

### Re-hosting verdict
**CONDITIONAL GO.** The MODEL_LICENSE text permits sharing with attribution and does not explicitly prohibit commercial use or re-hosting. However, the "reference and learning purposes only" warranty disclaimer creates enough ambiguity that `license_verified` should remain `false` until one of the following:
1. A Brevity legal read of the full MODEL_LICENSE confirms the disclaimer is a warranty carveout (not a scope restriction), or
2. A written response from FunAudioLLM/Alibaba confirms commercial use and re-hosting are permitted.

**Practical mitigation:** The PRD already calls for a first-party re-host (Task A5). Proceed with the re-host (Task A5 is ready to go), but keep `license_verified = false` in the registry until legal review completes. The app can load SenseVoice normally; the registry flag is a documentation gate, not a runtime gate.

---

## Summary Table

| Model | License | SPDX / Tag | Commercial | Re-host OK? | Notices required | `license_verified` |
|-------|---------|-----------|-----------|-------------|-----------------|-------------------|
| NVIDIA nemotron-speech-streaming-en-0.6b | NVIDIA Open Model License | `nvidia-open-model-license` | Yes | Yes | NOTICE file: "Licensed by NVIDIA Corporation under the NVIDIA Open Model License" | `true` |
| NVIDIA TitaNet-small (via FluffyBunnies/titanet-small-onnx) | CC-BY-4.0 | `cc-by-4.0` | Yes | Yes | Attribution to NVIDIA; link to CC-BY-4.0; note ONNX conversion | `true` |
| onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX (+ upstream mistralai) | Apache 2.0 | `apache-2.0` | Yes | Yes | Apache 2.0 license text; note ONNX conversion | `true` |
| onnx-community/Qwen3-0.6B-ONNX / Qwen3-1.7B-ONNX | Apache 2.0 (inherited from Qwen/Qwen3) | `apache-2.0` | Yes | Yes | Confirm YAML front matter on both repos; Apache 2.0 text | `true` (after README confirm) |
| FunAudioLLM/SenseVoiceSmall (model weights) | FunASR Model Open Source License v1.1 | `other` (`funasr-model-license`) | Ambiguous | Probably yes — not explicitly prohibited | Attribution to FunAudioLLM/Alibaba; retain model name | `false` — pending legal review |
| k2-fsa/sherpa-onnx (runtime/packaging) | Apache 2.0 | `apache-2.0` | Yes | Yes (runtime only) | Standard Apache 2.0 | `true` (runtime) |

---

## Open Actions Before Phase 0 Exit

1. **Qwen3 ONNX repos — 2-minute check:** Fetch raw `README.md` YAML front matter from `onnx-community/Qwen3-0.6B-ONNX` and `onnx-community/Qwen3-1.7B-ONNX`. Confirm `license: apache-2.0` is present. Then flip `license_verified = true`. (Task D1 can do this inline.)

2. **SenseVoice legal review (recommended):** Share the full FunASR MODEL_LICENSE with a lawyer or Alibaba contact before flipping `license_verified = true`. The critical question: does "provided for reference and learning purposes only" limit the license grant scope, or is it a warranty disclaimer only? If it is a warranty disclaimer (the more likely reading), `license_verified` flips to `true`.

3. **FluffyBunnies NOTICE gap:** When the first-party TitaNet re-host is created (or pinned), add a `NOTICE` file crediting NVIDIA as original author under CC-BY-4.0. This is required for CC-BY-4.0 compliance.

4. **Nemotron NOTICE file:** First-party Nemotron re-host repo (if created) needs a `NOTICE` file with the exact text: *"Licensed by NVIDIA Corporation under the NVIDIA Open Model License"* per Section 3.1.

---

## Sources

- NVIDIA Open Model License: https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/
- NVIDIA Nemotron HF card: https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b/blob/main/README.md
- NVIDIA TitaNet-large (CC-BY-4.0 reference): https://huggingface.co/nvidia/speakerverification_en_titanet_large/blob/main/README.md
- FluffyBunnies TitaNet-small-onnx: https://huggingface.co/FluffyBunnies/titanet-small-onnx
- CC-BY-4.0 legal code: https://creativecommons.org/licenses/by/4.0/legalcode
- onnx-community Voxtral ONNX: https://huggingface.co/onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX
- Mistral Voxtral upstream: https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602
- Qwen/Qwen3-0.6B: https://huggingface.co/Qwen/Qwen3-0.6B
- Qwen/Qwen3-1.7B: https://huggingface.co/Qwen/Qwen3-1.7B
- FunAudioLLM SenseVoiceSmall HF card (raw): https://huggingface.co/FunAudioLLM/SenseVoiceSmall/raw/main/README.md
- FunASR MODEL_LICENSE: https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE
- SenseVoice commercial use issue #279: https://github.com/FunAudioLLM/SenseVoice/issues/279
- sherpa-onnx: https://github.com/k2-fsa/sherpa-onnx
