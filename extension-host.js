/**
 * extension-host.js — the sandboxed extension host runtime (PRD Phase 6, Task
 * J2; R7).
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/silent_web.js`). It exposes an `ExtensionHost` that
 * `index.html` drives — the same strangler-fig "one wasm binary, many surfaces"
 * pattern as `storage-engine.js` / `session-engine.js`. The wasm core is the
 * POLICY (manifest validation, grant-set persistence, the per-extension
 * data/UI/network boundary checks, the versioned envelope); this glue is the
 * HANDS (the sandboxed iframe per extension, the `postMessage` plumbing, the
 * panel/notification rendering, the consent + manager UI hooks).
 *
 * # Isolation model — sandboxed iframe, NOT a bare Worker. WHY:
 *
 * `docs/EXTENSIONS.md` §3 prefers a Web Worker "or a sandboxed iframe", and §1.2
 * requires that the `panel` UI capability render HTML "inside the panel iframe,
 * not injected into the main page". An extension needs BOTH code execution AND a
 * render surface. A bare Worker has no DOM, so a Worker-only design would force
 * the host to inject extension-authored HTML into the main page — exactly what
 * §5 `render.panel` forbids. A **sandboxed iframe** (`sandbox="allow-scripts"`,
 * crucially WITHOUT `allow-same-origin`) gives us, in one primitive:
 *
 *   - a NULL-origin document: no access to the host `window`, `document`,
 *     `localStorage`, IndexedDB, cookies, or the host's `crossOriginIsolated`
 *     SharedArrayBuffer — true origin isolation (§3 "no access to the host page's
 *     window/document/globalThis", "no SharedArrayBuffer", "no IndexedDB except
 *     through the host's message API");
 *   - script execution for the extension's message handler;
 *   - a native render surface the extension owns, so the host NEVER injects
 *     extension HTML into the main page (the panel is the extension's own
 *     sandboxed DOM — the host only sizes the slot).
 *
 * The iframe is `srcdoc`-booted with a fixed bootstrap that (a) wires the single
 * `postMessage` channel and (b) `import()`s the extension's entrypoint as a
 * module. No `allow-same-origin` ⇒ the iframe cannot reach back into the host.
 * The ONLY channel is `postMessage`, exactly as §3 requires.
 *
 * # The data boundary is enforced in Rust, not here
 *
 * Every outbound host→extension message is gated by `gateHostMessage` (wasm): if
 * the relevant capability is not in the persisted grant set, the call returns
 * `null` and we send NOTHING — ungranted data is silently omitted, the extension
 * never learns what it did not declare (`docs/EXTENSIONS.md` §2). Every inbound
 * extension→host message arrives as a versioned envelope and is validated by
 * `readExtensionEnvelope` (wasm), which REFUSES a protocol-version mismatch (a
 * forged v2 envelope is rejected). UI requests are then re-checked against the
 * grant set (`hasUiGrant`) before the host acts.
 *
 * Privacy: no audio, embeddings, or model tensors can cross this surface — they
 * are not representable in the capability vocabulary OR the message protocol
 * (enforced by the absence of a type in `silent-extension-sdk`). This glue moves
 * only the transcript text / notes / speaker labels / meeting metadata the user
 * already sees, and only the subset each extension was granted.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

/**
 * Shared, cross-loader module-init promise for the wasm-pack pkg (see
 * session-engine.js for the full rationale): one `import()` + `default()` across
 * ALL engine loaders, keyed by pkg URL, so a concurrent boot-time init never
 * double-initializes the wasm binary and corrupts the heap.
 */
function _loadModule(pkgUrl) {
  const w = (typeof window !== 'undefined') ? window : globalThis;
  const cache = (w.__silentWebModulePromises ||= new Map());
  let p = cache.get(pkgUrl);
  if (!p) {
    p = (async () => {
      const mod = await import(pkgUrl);
      await mod.default();   // initialises the wasm binary exactly once
      return mod;
    })();
    cache.set(pkgUrl, p);
  }
  return p;
}

/**
 * The iframe bootstrap. Runs INSIDE the null-origin sandbox. It is the ONLY
 * scaffolding the host injects; the extension's real logic (`entrypointSource`)
 * is INLINED as a second module so the sandbox never has to fetch a cross-origin
 * module — a null-origin (opaque) `srcdoc` document cannot `import()` a
 * cross-origin URL under COEP=credentialless, so the host fetches the source
 * (same-origin, which it is allowed to) and embeds it here. The bootstrap:
 *   - listens on the single `postMessage` channel from the host,
 *   - exposes a minimal `silent` API the extension uses to post messages back,
 *   - forwards host messages to the extension's `onHostMessage` handler,
 *   - then runs the inlined extension module, with `globalThis.silent` ready.
 *
 * The extension cannot reach the host (no allow-same-origin); it can only post
 * messages, which the host validates.
 */
function _escapeScript(src) {
  // Prevent the inlined source from breaking out of its <script> element.
  return String(src).replace(/<\/script/gi, '<\\/script').replace(/<!--/g, '<\\!--');
}

function _bootstrapHtml(extensionId, entrypointSource) {
  // NOTE: this string is the iframe's whole document. It contains NO host data.
  // The extension source is inlined; it never sees host globals or the host DOM.
  return `<!DOCTYPE html><html><head><meta charset="utf-8">
<style>
  :root { color-scheme: dark; }
  html,body { margin:0; padding:0; background:#12121a; color:#e0e0e8;
    font:13px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',system-ui,sans-serif; }
  #ext-root { padding:10px 12px; }
  a { color:#00d4aa; }
</style></head>
<body><div id="ext-root"></div>
<script>
  // Bootstrap (classic script so it runs synchronously BEFORE the extension
  // module below). Defines globalThis.silent and the single host channel, and
  // BUFFERS host messages until the extension registers its handler — so the
  // extension never misses a message regardless of module-eval timing.
  (function () {
    var EXT_ID = ${JSON.stringify(extensionId)};
    var _handler = null;
    var _buffer = [];   // host messages received before onHostMessage()
    function _deliver(message) {
      if (typeof _handler === 'function') {
        try { _handler(message); }
        catch (err) { console.error('[ext:' + EXT_ID + '] handler error', err); }
      } else {
        _buffer.push(message);
      }
    }
    globalThis.silent = {
      extensionId: EXT_ID,
      // Register the host→extension handler; flush any buffered messages into it.
      onHostMessage: function (fn) {
        _handler = fn;
        var pending = _buffer; _buffer = [];
        for (var i = 0; i < pending.length; i++) _deliver(pending[i]);
      },
      // Send an ExtensionMessage to the host (render.panel / render.notification /
      // export.request). The host wraps + validates each one.
      post: function (message) { parent.postMessage({ __silentExt: true, message: message }, '*'); },
      // Render HTML into OUR OWN sandbox document (never the host page).
      renderLocal: function (html) {
        var root = document.getElementById('ext-root');
        if (root) root.innerHTML = String(html);
      },
    };
    window.addEventListener('message', function (ev) {
      var data = ev.data;
      if (!data) return;
      // Host-echoed panel HTML (validated against the panel grant): render it
      // into our OWN sandbox document.
      if (data.__silentHostRender) {
        var root = document.getElementById('ext-root');
        if (root) root.innerHTML = String(data.html || '');
        return;
      }
      // A versioned host→extension envelope { protocolVersion, extensionId,
      // message }: deliver (or buffer) the INNER message body to the extension.
      if (data.__silentHost) {
        var env = data.message;
        _deliver(env && env.message ? env.message : env);
      }
    });
    // Ready immediately: the buffer guarantees no message is lost before the
    // extension module (below) registers its handler.
    parent.postMessage({ __silentExtReady: true, extensionId: EXT_ID }, '*');
  })();
</script>
<!-- The extension's own module, inlined (no cross-origin import from this
     opaque-origin sandbox). It sees globalThis.silent but no host globals. -->
<script type="module">
try {
${_escapeScript(entrypointSource)}
} catch (err) {
  console.error('[ext] entrypoint threw', err);
  var root = document.getElementById('ext-root');
  if (root) root.textContent = 'Extension failed to run: ' + (err && err.message || err);
}
</script></body></html>`;
}

/**
 * One installed, running extension: its grant set + its sandboxed iframe.
 */
class ExtensionInstance {
  constructor(grantSet, iframe) {
    this.grantSet = grantSet;              // the parsed GrantSet (from wasm)
    this.grantJson = JSON.stringify(grantSet);
    this.iframe = iframe;                  // the sandboxed <iframe> element
    this.ready = false;                    // set true on __silentExtReady
    this._pending = [];                    // host messages queued until ready
  }
  get name() { return this.grantSet.extension; }
}

export class ExtensionHost {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]   Override for the wasm-pack pkg URL.
   * @param {Element} [opts.panelSlot]  The DOM element the panel iframes mount into.
   * @param {(text:string)=>void} [opts.onNotification]  Toast sink (host-owned).
   * @param {(snapshot:object, ext:ExtensionInstance)=>object} [opts.fillExport]
   *        Host callback: given the granted-surface flags, returns the actual
   *        { transcript, notes, speakers } the host is willing to share. The host
   *        owns the live data; the wasm policy only says WHICH surfaces.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;
    this.panelSlot = opts.panelSlot || null;
    this.onNotification = opts.onNotification || ((t) => console.log('[ext-toast]', t));
    this.fillExport = opts.fillExport || (() => ({}));

    this._mod = null;
    this._loadPromise = null;
    this.ready = false;
    /** @type {Map<string, ExtensionInstance>} name → running instance */
    this.instances = new Map();

    this._onWindowMessage = this._onWindowMessage.bind(this);
  }

  load() {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;
    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      window.addEventListener('message', this._onWindowMessage);
      this.ready = true;
      console.log('[rust-ext] ExtensionHost ready (sandboxed-iframe host, protocol v' + this._mod.protocolVersion() + ')');
    })();
    return this._loadPromise;
  }

  _m() {
    if (!this._mod) throw new Error('[rust-ext] ExtensionHost not loaded — call load() first');
    return this._mod;
  }

  // ── Install / consent ───────────────────────────────────────────────────

  /**
   * Validate a manifest for the consent screen. Returns the PARSED manifest
   * object (name, displayName, version, capabilities, …) the UI renders the
   * consent summary from. THROWS the precise ManifestError string verbatim if
   * the manifest is invalid (unknown capability, wildcard origin, oversize, bad
   * entrypoint) — the consent UI shows that string verbatim.
   * @param {string} manifestJson
   * @returns {object} parsed Manifest
   */
  validateManifest(manifestJson) {
    return JSON.parse(this._m().validateManifest(String(manifestJson)));
  }

  /**
   * Approve + persist an install (the consent "Allow"). Re-validates, grants
   * ALL the manifest's declared capabilities, persists the grant set, then
   * mounts the extension's sandboxed iframe. `manifestDir` is the absolute base
   * URL the entrypoint resolves against.
   * @param {string} manifestJson
   * @param {string} manifestDir   absolute URL of the extension's directory
   * @returns {Promise<ExtensionInstance>}
   */
  async install(manifestJson, manifestDir) {
    const grantJson = await this._m().commitInstall(String(manifestJson));
    const grantSet = JSON.parse(grantJson);
    const manifest = JSON.parse(manifestJson);
    return await this._mount(grantSet, manifest, manifestDir);
  }

  /**
   * Re-hydrate every installed extension on boot: load each persisted grant set
   * and mount its iframe. The manifest + dir for each are looked up from the
   * provided registry map (name → { manifestJson, dir }).
   * @param {Object<string,{manifestJson:string,dir:string}>} registry
   * @returns {Promise<ExtensionInstance[]>}
   */
  async rehydrate(registry) {
    const sets = JSON.parse(await this._m().loadAllGrantSets());
    const out = [];
    for (const grantSet of sets) {
      const entry = registry[grantSet.extension];
      if (!entry) continue;   // installed but its code is no longer present
      try {
        out.push(await this._mount(grantSet, JSON.parse(entry.manifestJson), entry.dir));
      } catch (err) {
        console.warn('[rust-ext] re-mount failed for', grantSet.extension, err);
      }
    }
    return out;
  }

  /** Load one extension's persisted grant set (or null). */
  async loadGrantSet(name) {
    const json = await this._m().loadGrantSet(String(name));
    return json ? JSON.parse(json) : null;
  }

  /** Load every installed extension's grant set. */
  async loadAllGrantSets() {
    return JSON.parse(await this._m().loadAllGrantSets());
  }

  /** The per-extension connect-src origins (j3 applies them to the CSP). */
  connectSrc(grantSet) {
    return JSON.parse(this._m().connectSrc(JSON.stringify(grantSet)));
  }

  /**
   * Revoke + remove an extension: delete its grant set and tear down its iframe.
   * @param {string} name
   */
  async revoke(name) {
    await this._m().revokeExtension(String(name));
    const inst = this.instances.get(name);
    if (inst) {
      if (inst.iframe && inst.iframe.parentNode) inst.iframe.parentNode.removeChild(inst.iframe);
      this.instances.delete(name);
    }
  }

  // ── Mounting the sandbox ────────────────────────────────────────────────

  async _mount(grantSet, manifest, manifestDir) {
    const name = grantSet.extension;
    // Replace any prior instance.
    if (this.instances.has(name)) {
      const old = this.instances.get(name);
      if (old.iframe && old.iframe.parentNode) old.iframe.parentNode.removeChild(old.iframe);
      this.instances.delete(name);
    }

    // Fetch the entrypoint SOURCE same-origin (the host is allowed to), to inline
    // it into the sandbox. A null-origin srcdoc iframe cannot import() a
    // cross-origin module under COEP, so we embed the code rather than load it.
    const entrypointUrl = new URL(manifest.entrypoint, manifestDir).href;
    const res = await fetch(entrypointUrl);
    if (!res.ok) throw new Error('entrypoint ' + manifest.entrypoint + ' HTTP ' + res.status);
    const entrypointSource = await res.text();

    const iframe = document.createElement('iframe');
    // The hard isolation: allow-scripts but NOT allow-same-origin → null origin.
    iframe.setAttribute('sandbox', 'allow-scripts');
    iframe.setAttribute('title', 'Extension: ' + (manifest.displayName || name));
    iframe.className = 'ext-panel-frame';
    iframe.dataset.extension = name;
    iframe.srcdoc = _bootstrapHtml(name, entrypointSource);

    const inst = new ExtensionInstance(grantSet, iframe);
    this.instances.set(name, inst);
    if (this.panelSlot) this.panelSlot.appendChild(iframe);
    return inst;
  }

  // ── Outbound host → extension (gated) ───────────────────────────────────

  /**
   * Broadcast one HostMessage to every installed extension, gated per-extension
   * by its grant set. An extension that lacks the required capability receives
   * NOTHING (the wasm gate returns null). `message` is a plain HostMessage body
   * object: { type, payload }.
   * @param {object} message
   */
  broadcast(message) {
    for (const inst of this.instances.values()) this.send(inst.name, message);
  }

  /**
   * Send one HostMessage to one extension, gated by its grant set. Returns true
   * if the message was actually delivered (capability granted), false if it was
   * silently omitted.
   * @param {string} name
   * @param {object} message  HostMessage body { type, payload }
   * @returns {boolean}
   */
  send(name, message) {
    const inst = this.instances.get(name);
    if (!inst) return false;
    // The wasm gate: returns the versioned envelope JSON, or null if ungranted.
    const enveloped = this._m().gateHostMessage(inst.grantJson, JSON.stringify(message));
    if (!enveloped) return false;   // ungranted → silently omitted
    const envelope = JSON.parse(enveloped);
    this._postToFrame(inst, envelope);
    return true;
  }

  _postToFrame(inst, envelope) {
    const deliver = () => inst.iframe.contentWindow &&
      inst.iframe.contentWindow.postMessage({ __silentHost: true, message: envelope }, '*');
    if (inst.ready) deliver();
    else inst._pending.push(deliver);
  }

  // ── Inbound extension → host (validated) ────────────────────────────────

  _onWindowMessage(ev) {
    const data = ev.data;
    if (!data) return;

    // An extension iframe announcing it is ready: flush any queued host messages.
    if (data.__silentExtReady) {
      const inst = this.instances.get(data.extensionId);
      if (inst && ev.source === inst.iframe.contentWindow) {
        inst.ready = true;
        const q = inst._pending.splice(0);
        for (const fn of q) fn();
      }
      return;
    }

    if (!data.__silentExt) return;   // not one of ours

    // Identify which instance this came from by matching the source window.
    let inst = null;
    for (const candidate of this.instances.values()) {
      if (candidate.iframe.contentWindow === ev.source) { inst = candidate; break; }
    }
    if (!inst) return;   // message from an unknown/destroyed frame — ignore

    // Wrap the raw extension message into a versioned envelope and validate it
    // through the wasm policy (this is also where a forged protocol version
    // would be REFUSED — see readExtensionEnvelope; here we build our own
    // envelope at the host's version, so a same-origin extension cannot forge a
    // version, but an export.response forgery or a future malformed body still
    // routes through the typed decode).
    let inbound;
    try {
      const envelope = {
        protocolVersion: this._m().protocolVersion(),
        extensionId: inst.name,
        message: data.message,
      };
      inbound = JSON.parse(this._m().readExtensionEnvelope(JSON.stringify(envelope)));
    } catch (err) {
      console.warn('[rust-ext] rejected extension message from "' + inst.name + '":', err.message || err);
      return;
    }

    this._handleInbound(inst, inbound.message);
  }

  _handleInbound(inst, message) {
    if (!message || typeof message.type !== 'string') return;
    switch (message.type) {
      case 'render.panel': {
        // Re-check the panel UI grant (silent no-op if not granted).
        if (!this._m().hasUiGrant(inst.grantJson, 'panel')) return;
        // The host does NOT inject this HTML into the main page. It echoes the
        // HTML back into the extension's OWN sandbox document, where the
        // bootstrap writes it under the null-origin iframe (docs §5: panel
        // content is rendered inside the panel iframe, not the main page).
        const html = (message.payload && message.payload.html) || '';
        if (inst.iframe.contentWindow) {
          inst.iframe.contentWindow.postMessage({ __silentHostRender: true, html: String(html) }, '*');
        }
        return;
      }
      case 'render.notification': {
        if (!this._m().hasUiGrant(inst.grantJson, 'notification')) return;
        const text = (message.payload && message.payload.text) || '';
        this.onNotification(String(text));
        return;
      }
      case 'export.request': {
        const include = (message.payload && message.payload.include) || [];
        // The wasm policy returns the GRANTED subset of requested surfaces.
        const surfaces = JSON.parse(
          this._m().grantedExportSurfaces(inst.grantJson, JSON.stringify(include)));
        // The host fills ONLY those surfaces from its live data.
        const snapshot = this.fillExport(surfaces, inst) || {};
        // Wrap as a versioned export.response and deliver.
        const enveloped = this._m().wrapExportResponse(inst.name, JSON.stringify(snapshot));
        this._postToFrame(inst, JSON.parse(enveloped));
        return;
      }
      default:
        // Unknown/future request type — ignore (deny by default).
        return;
    }
  }
}
