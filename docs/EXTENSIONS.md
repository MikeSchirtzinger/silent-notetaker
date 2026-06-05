# Extension System Design

> **Status: DESIGN — not yet implemented.**
> This document specifies the extension system planned for Phase 3 of the
> [migration roadmap](ARCHITECTURE.md#7-migration-roadmap). Nothing described
> here ships today. It is written now so the community can reason about the
> direction, and so that Phase 3 has a concrete spec to implement against.

---

## The threat this sandbox exists to solve

**A third-party extension marketplace is in direct conflict with the core
promise.**

Silent Notetaker's entire value proposition is "private by architecture, not by
policy" — the audio physically cannot leave your device, and anyone can verify
that. The moment a community extension is allowed to run arbitrary code alongside
that audio, the guarantee degrades from "enforced" to "trust that no one
installed something sketchy." That is exactly the policy-based model we are
trying to make obsolete.

**The sandbox is therefore a hard requirement, not a product nice-to-have.** If
the isolation is not real — if an extension can smuggle audio, embeddings, or
transcript data to a server the user did not explicitly approve — then the
marketplace destroys the only thing that makes this product different from
Granola.

The three controls that make a privacy-first extension ecosystem defensible:

1. **Network denied by default.** An extension cannot open a socket or `fetch`
   to *any* host until the user explicitly grants it. The grant is named,
   host-scoped, and revocable.
2. **Isolated execution.** Extensions run in a Worker or sandboxed iframe. They
   have no access to the host page's DOM, globals, or memory — only what the
   host deliberately sends across a narrow `postMessage` channel.
3. **Declared capabilities.** A manifest states exactly what data and UI surface
   an extension may access. Anything not declared is denied at the boundary, not
   just by convention.

---

## 1. Manifest schema

Every extension is described by a `manifest.json` at its root:

```jsonc
{
  "name": "notion-export",          // machine-readable id; no spaces
  "displayName": "Notion Export",   // shown in the extension manager UI
  "version": "0.2.1",               // semver
  "description": "Push decisions and action items to a Notion page.",
  "entrypoint": "index.js",         // relative path; loaded in a Worker
  "capabilities": {
    "data":    ["transcript.text", "notes.decisions", "notes.actions"],
    "ui":      ["panel"],
    "network": ["https://api.notion.com"]
  }
}
```

### 1.1 `capabilities.data` — what the extension may read

The host enforces this list; undeclared fields are stripped before any message
crosses the boundary. Raw audio and raw embeddings are **never** on this list —
they are not in the vocabulary of the extension API at all.

| Capability token | What the extension receives |
|---|---|
| `transcript.text` | The running transcript as plain text segments, timestamped |
| `transcript.segments` | Segments with speaker label, start/end timestamps, and confidence |
| `notes.decisions` | Extracted decisions (text only) |
| `notes.actions` | Extracted action items (text + speaker attribution if available) |
| `notes.keypoints` | Extracted key points |
| `notes.questions` | Open questions surfaced during the meeting |
| `speaker.labels` | The current `S1 → "Alice"` rename map |
| `meeting.metadata` | Title, start time, duration — no audio, no embeddings |

**Never exposed — not grantable, not present in any API surface:**

- Raw audio samples (PCM, encoded, or otherwise)
- Raw voice embeddings (the float vectors TitaNet produces)
- The mel-spectrogram tensors
- Any intermediate model activations
- IndexedDB data from meetings the extension was not active for

### 1.2 `capabilities.ui`

| Token | What it allows |
|---|---|
| `panel` | Render a side panel inside the Silent Notetaker UI (sandboxed iframe) |
| `notification` | Post a short in-app toast notification (text only, no HTML) |

Extensions do not get free-form DOM injection. A panel is an iframe with a
fixed slot; the host controls the slot.

### 1.3 `capabilities.network`

- Default: **empty** — no network access at all.
- Each entry is a full origin (`https://api.notion.com`), not a wildcard.
- The list is shown to the user at install time and must be approved.
- Approvals are stored locally and revocable from the extension manager UI.
- The browser-level CSP `connect-src` is dynamically updated to include approved
  origins for that extension's Worker/iframe and nowhere else.

---

## 2. Permission model

### Install-time consent

When a user installs an extension the host presents a permission summary screen:

```
"Notion Export" wants to:
  • Read your transcript text, decisions, and action items
  • Show a side panel
  • Send data to api.notion.com

[Allow]  [Deny]
```

No installation proceeds without explicit allow. The grant is recorded locally
(IndexedDB, same store as meeting data). Nothing is sent to any remote registry.

### Runtime enforcement

- **Data capability** — the host checks declared `capabilities.data` before
  dispatching any message. Undeclared fields are omitted from the payload,
  silently. The extension never knows what it did not declare.
- **Network capability** — the extension Worker/iframe runs under a per-extension
  CSP that only adds the approved origins to `connect-src`. The base page CSP
  (from `_headers` / the axum server) is not relaxed.
- **UI capability** — if `panel` is not declared, the extension's render requests
  are ignored. If `notification` is not declared, toast calls are no-ops.

### Revocation

Users can open the extension manager, inspect current grants, and revoke any
permission. Revocation takes effect immediately: the next time the host dispatches
a message it re-checks stored grants. A revoked network grant is also removed from
the per-extension CSP before the next Worker restart.

---

## 3. Isolation model

### Execution context

> **Implemented (Phase 6 / J2): the sandboxed iframe, not the bare Worker.**
> The host (`extension-host.js` + `silent-web`'s `extension_host` surface) runs
> each extension in a **null-origin sandboxed iframe** (`sandbox="allow-scripts"`,
> deliberately *without* `allow-same-origin`). The Worker was the original
> preference, but the `panel` UI capability (§1.2, §5 `render.panel`) requires a
> render surface, and a bare Worker has no DOM — a Worker-only design would force
> the host to inject extension-authored HTML into the main page, exactly what §5
> forbids. A sandboxed iframe gives true origin isolation *and* a render surface
> the extension owns, in one primitive. Because a null-origin (opaque) `srcdoc`
> document cannot `import()` a cross-origin module under COEP=credentialless, the
> host fetches the entrypoint **source** same-origin and *inlines* it into the
> sandbox bootstrap, rather than loading it by URL. The single `postMessage`
> channel and the versioned envelope are unchanged.

An extension runs in a **Web Worker** (preferred) or a **sandboxed iframe**
(`sandbox="allow-scripts"`, no `allow-same-origin`). It has:

- No access to the host page's `window`, `document`, or `globalThis` other than
  the Worker's own scope.
- No `SharedArrayBuffer` (cross-origin isolation is set to allow this for the
  main app, not granted to extension Workers).
- No access to the `AudioContext`, microphone, or any media device.
- No access to IndexedDB except through the host's message API (which only returns
  data the extension has declared and the user has approved).

### Communication channel

The only channel between the host and an extension is `postMessage`. The host
holds a reference to the Worker (or the iframe's `contentWindow`); the extension
holds a reference to nothing — it only receives messages and posts replies. There
is no shared memory.

---

## 4. Host-to-extension messages

The host emits these message types on the channel. An extension declares which
ones it cares about by listing the corresponding `capabilities.data` tokens.

All message objects share this envelope:

```jsonc
{
  "type":      "<message type>",
  "extensionId": "<name from manifest>",
  "payload":   { /* type-specific */ }
}
```

### `transcript.update`

Sent after each committed transcription segment.

```jsonc
{
  "type": "transcript.update",
  "payload": {
    "segmentId":  "seg-042",
    "text":       "We should ship the CSP change before the launch.",
    "speaker":    "Alice",          // resolved from the speaker.labels map
    "speakerRaw": "S1",
    "startMs":    183400,
    "endMs":      187200,
    "confidence": 0.94
  }
}
```

Requires `transcript.text` or `transcript.segments` in `capabilities.data`.

### `notes.update`

Sent when the `NoteEngine` emits a new note of any category.

```jsonc
{
  "type": "notes.update",
  "payload": {
    "noteId":   "note-017",
    "category": "decision",        // "decision" | "action" | "keypoint" | "question"
    "text":     "Ship the CSP change before the HN launch.",
    "speaker":  "Alice",
    "timestampMs": 187200
  }
}
```

Requires the corresponding `notes.*` capability token.

### `speaker.rename`

Sent when a user renames a speaker (e.g. "S1" → "Alice").

```jsonc
{
  "type": "speaker.rename",
  "payload": {
    "raw":     "S1",
    "display": "Alice"
  }
}
```

Requires `speaker.labels`.

### `meeting.start`

Sent when the user clicks Start.

```jsonc
{
  "type": "meeting.start",
  "payload": {
    "meetingId": "m-20260602-143012",
    "title":     "HN prep stand-up",
    "startMs":   1748872212000
  }
}
```

Requires `meeting.metadata`.

### `meeting.stop`

Sent when the user clicks Stop. Carries a summary of the session so far.

```jsonc
{
  "type": "meeting.stop",
  "payload": {
    "meetingId":  "m-20260602-143012",
    "durationMs": 2847000,
    "speakerMap": { "S1": "Alice", "S2": "Bob" },
    "noteCounts": { "decisions": 3, "actions": 7, "keypoints": 5, "questions": 2 }
  }
}
```

Requires `meeting.metadata`.

---

## 5. Extension-to-host requests

An extension may send these message types back to the host. The host validates
every request against the manifest before acting.

### `render.panel`

Ask the host to mount the extension's panel content.

```jsonc
{
  "type":    "render.panel",
  "payload": { "html": "<p>Notion status: connected</p>" }
}
```

The `html` string is rendered inside the panel iframe, not injected into the main
page. Requires `panel` in `capabilities.ui`. The host sanitizes the HTML before
display.

### `render.notification`

Post a short toast.

```jsonc
{
  "type":    "render.notification",
  "payload": { "text": "3 action items pushed to Notion." }
}
```

Requires `notification` in `capabilities.ui`.

### `export.request`

Ask the host for a snapshot of the meeting data the extension is entitled to.
This is the pull alternative to the push messages above — useful on `meeting.stop`
to grab everything at once.

```jsonc
{
  "type":    "export.request",
  "payload": {
    "include": ["transcript.text", "notes.decisions", "notes.actions"]
  }
}
```

The host responds with an `export.response` message containing only the
intersection of `include` and the extension's declared `capabilities.data`.
Anything outside that intersection is omitted without error.

---

## 6. Worked example: `notion-export`

This example shows a complete extension — manifest plus message handler — end to
end. It is illustrative; the exact Worker bootstrap API is part of the Phase 3
implementation and may differ in detail.

### `manifest.json`

```jsonc
{
  "name": "notion-export",
  "displayName": "Notion Export",
  "version": "0.1.0",
  "description": "Push decisions and action items to a Notion database page.",
  "entrypoint": "index.js",
  "capabilities": {
    "data":    ["notes.decisions", "notes.actions", "meeting.metadata"],
    "ui":      ["panel", "notification"],
    "network": ["https://api.notion.com"]
  }
}
```

### `index.js`

```js
// notion-export/index.js  (~30 lines, runs in a Worker)
// The host posts messages here; we reply or call the Notion API.

const NOTION_TOKEN = self.name;   // host injects config via Worker name at init (future API)
const DATABASE_ID  = "abc123";    // user-configured; shown in the panel settings form

self.onmessage = async ({ data: msg }) => {
  switch (msg.type) {

    case "meeting.start":
      // Show a connected indicator in our side panel
      post("render.panel", { html: `<p>Notion Export: ready for meeting ${msg.payload.meetingId}</p>` });
      break;

    case "meeting.stop": {
      // On stop: grab the full note set and push to Notion
      post("export.request", { include: ["notes.decisions", "notes.actions"] });
      break;
    }

    case "export.response": {
      const { decisions = [], actions = [] } = msg.payload;
      const blocks = [
        ...decisions.map(d => bulletBlock("decision", d.text)),
        ...actions.map(a =>   bulletBlock("action",   a.text)),
      ];
      const res = await fetch("https://api.notion.com/v1/blocks/" + DATABASE_ID + "/children", {
        method:  "PATCH",
        headers: { "Authorization": "Bearer " + NOTION_TOKEN, "Notion-Version": "2022-06-28",
                   "Content-Type": "application/json" },
        body: JSON.stringify({ children: blocks }),
      });
      const ok = res.ok;
      post("render.notification", { text: ok ? `${blocks.length} items pushed to Notion.`
                                              : "Notion push failed — check console." });
      post("render.panel", { html: `<p>${ok ? "Synced" : "Error"}: ${new Date().toLocaleTimeString()}</p>` });
      break;
    }
  }
};

function post(type, payload) {
  self.postMessage({ type, payload });
}

function bulletBlock(prefix, text) {
  return {
    object: "block", type: "bulleted_list_item",
    bulleted_list_item: { rich_text: [{ type: "text", text: { content: `[${prefix}] ${text}` } }] }
  };
}
```

**What this extension can do:**
- Read decisions and actions extracted by the NoteEngine.
- Post to `api.notion.com` (user-approved at install).
- Render a status panel and post toast notifications.

**What this extension cannot do:**
- Access raw audio, embeddings, or any model activations.
- Reach any host other than `api.notion.com`.
- Inject HTML into the main page.
- Read past meetings it was not active for.
- Store data in IndexedDB directly.

---

## 7. What is shipped today vs. what this document designs

| Item | Status |
|---|---|
| Single-file monolith | Split (Phase 1) |
| CSP enforcement | **ENFORCED** (J3) — base page CSP is `Content-Security-Policy` (no longer report-only), `script-src` includes `'wasm-unsafe-eval'` for the wasm-pack engines; `--report-only` flag retained for rollback. The per-extension `connect-src` is derived from grants (`GrantSet::connect_src`) and applied as a `<meta>` CSP inside each extension's sandboxed iframe. **Network DENY-by-default is fully enforced** (an undeclared fetch is blocked by CSP and logged to `ExtensionHost.cspViolations`). **Known limitation (J3 finding):** a `srcdoc`/`blob:` iframe *inherits the embedder's CSP* in Chromium, and CSP combines by intersection — so a child can only *tighten*, never *widen*, the base page `connect-src`. The per-extension network *grant* (relaxing `connect-src` to an approved origin) therefore does NOT take effect while extensions run in `srcdoc` iframes: a granted origin is still blocked by the inherited base policy. Making grants functional requires serving each extension from a distinct same-origin document whose *response header* carries only that extension's CSP (no inheritance) — a change to the J2 isolation/serving primitive, tracked for a follow-up. The privacy-critical guarantee (deny-by-default; base page never relaxed) is met today; grants are effectively still deny. |
| Extension manifest format | **Implemented** — `silent-extension-sdk` (J1) |
| Worker/iframe sandbox | **Implemented** — null-origin sandboxed iframe, inlined source (J2) |
| postMessage API + versioned envelope | **Implemented** — `extension_host` surface + `extension-host.js` (J2); version-mismatched envelopes refused |
| Grant-set persistence | **Implemented** — `extensionGrants` IndexedDB store, schema v4 (J2) |
| Install consent + manager UI | **Implemented** — consent screen (capabilities + network verbatim) + Settings manager (J2) |
| `reference-notes-export` reference extension | **Implemented** — `extensions/reference-notes-export/` (the R7 acceptance vehicle) |

The roadmap sequence is: Phase 1 (modularize) → Phase 2 (lazy + toggles, drop
`unsafe-inline`) → **Phase 3 (Extension SDK, this doc)** → Phase 4 (Tauri) →
Phase 5 (marketplace hosting). See [ARCHITECTURE.md §7](ARCHITECTURE.md#7-migration-roadmap)
for the full sequence and validation requirements.

The CSP `connect-src` floor described in [ARCHITECTURE.md §3](ARCHITECTURE.md#3-the-keystone-enforce-the-boundary-with-csp)
is the non-negotiable baseline that must be in place before any extension can be
installed. Extensions can only add hosts to their own Worker/iframe CSP; they
cannot relax the main page CSP.
