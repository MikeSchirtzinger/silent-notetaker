# Security Policy

Silent Notetaker's security model rests on a single promise: **your audio, voice
embeddings, and meeting content never leave your device except through explicit
actions you take**. This document describes what counts as a security vulnerability,
what the enforced boundaries are, and how to report a finding.

---

## What counts as a vulnerability

We are particularly interested in vulnerabilities that break the privacy boundary.
The following are the highest-priority classes:

### 1. Egress violations

Any path by which audio, transcript text, embeddings, mel tensors, logits, or
meeting notes reach a network destination that is not in the CSP `connect-src`
allowlist, or reach an allowed destination via an unauthorized channel.

Examples:
- A code path that serializes PCM audio into a `fetch` or WebSocket payload.
- A model host adapter that sends partial transcripts to a logging endpoint.
- An extension that exfiltrates data via an image `src`, form submit, or iframe
  navigation rather than a `fetch` (CSP bypasses, not just `connect-src`).
- A service worker that intercepts requests and proxies them to an unexpected host.

### 2. CSP bypasses

Any technique that circumvents the enforced Content-Security-Policy — particularly
the `connect-src` directive — allowing a page script, worker, or extension to
initiate a network connection to an origin not in the allowlist.

Examples:
- Injected script that opens a WebRTC data channel (not covered by `connect-src`).
- A `<meta http-equiv="refresh">` redirect to an unallowed origin.
- Prototype pollution that hijacks `fetch` before CSP applies.
- A JSONP-style script load via an allowed `script-src` host that then executes
  attacker-controlled code with network access.

### 3. Extension sandbox escapes

Any technique that allows an extension running in its sandboxed iframe to:
- Access the host page's DOM, globals, or in-memory state.
- Send or receive data outside its declared `capabilities.data` permissions.
- Access raw audio, embeddings, mel tensors, or model activations (which are not
  in the extension capability vocabulary and must never be reachable regardless of
  manifest declarations).
- Reach a network origin that was not granted by the user at install time.
- Read IndexedDB data from meetings the extension was not active for.

### 4. Raw-audio exposure

Any path that makes raw PCM samples, compressed audio bytes, mel spectrograms, or
voice embedding vectors available to:
- An extension (via postMessage or any side channel).
- The bridge WebSocket beyond transcript text.
- A `localStorage`, `sessionStorage`, or IndexedDB key readable by third-party code.
- A URL fragment, `history.pushState`, or other browser-observable state that could
  be intercepted.

### 5. Grant model bypass

Any technique that allows an extension to:
- Retain network access after the user revokes a grant.
- Gain network access to an origin it did not declare in its manifest, even if the
  user approved a different origin.
- Persist or escalate permissions beyond the `extensionGrants` IndexedDB store.

---

## What the enforced boundaries are

### Network boundary (CSP connect-src)

The base page CSP is enforced (not report-only). The allowlist is generated from
`registry/models.toml` by `cargo xtask gen-headers` and must not be modified by
hand. The current allowlist:

- `https://cdn.jsdelivr.net` — Transformers.js runtime
- `https://cdn.pyke.io` — onnxruntime-web loader
- `https://huggingface.co`, `https://*.hf.co`, `https://cdn-lfs.huggingface.co`,
  `https://cdn-lfs-us-1.huggingface.co` — model weight downloads
- `ws://localhost:8765` — optional Claude bridge (user's own machine only)
- `self`, `blob:`, `data:` — in-process resources

There is no analytics origin. There is no telemetry endpoint. Adding either would
require a registry entry and a regenerated `_headers`, both of which would be visible
in the diff and rejected by CI.

### Extension isolation boundary

- Extensions run in null-origin sandboxed iframes (`sandbox="allow-scripts"` without
  `allow-same-origin`).
- The `postMessage` channel is the only communication path.
- Raw audio, embeddings, mel tensors, and model activations are not variants of any
  capability enum in `silent-extension-sdk`. They cannot be requested in a manifest
  and cannot be sent across the boundary.
- Network grants are origin-scoped, user-approved at install, and revocable.

**Known pending limitation (J2b):** Per-extension `connect-src` relaxations (network
grants) are not yet functional in the `srcdoc` iframe context because Chromium applies
CSP by intersection with the embedder. The base page CSP is never relaxed; grants
are effectively deny until J2b is resolved. This is a known gap, not a vulnerability —
the privacy-critical direction (deny-by-default) is enforced; only the grant-relaxation
direction is pending.

### Audio boundary

Audio flows from the AudioWorklet through a `Float32RingBuffer` directly into the
WASM engine. It is never:
- Serialized to JSON or any structured format for transmission.
- Written to IndexedDB (only transcript chunks and metadata are stored).
- Made available to extension postMessage payloads.
- Passed to the bridge (the bridge receives transcript text only).

---

## How to report

**For privacy-boundary vulnerabilities (categories 1–5 above):** please email
`security@brevity.ventures` with a description of the vulnerability, steps to
reproduce, and the affected component. We will acknowledge within 48 hours and
aim to have a fix or mitigation plan within 7 days for confirmed critical issues.

**For non-privacy security issues** (e.g. dependency vulnerabilities in dev tooling,
XSS in the extension panel sandbox that does not escape to the host page): open a
GitHub issue with the label `security`. If in doubt, use the email path.

**Please do not open a public GitHub issue for privacy-boundary vulnerabilities**
before we have had a chance to assess and mitigate. We will coordinate disclosure
timing with you.

---

## What we ask in return

- Give us a reasonable time to respond before public disclosure.
- Do not exfiltrate or retain other users' data if you discover a vulnerability in
  a shared context (there is no shared server context in this app, but the principle
  stands).
- Test against your own browser instance, not against other users.

We appreciate responsible disclosure and will credit researchers who report valid
findings (unless they prefer to remain anonymous).
