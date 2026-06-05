/**
 * Cloudflare Pages Function — per-extension document route (SKETCH, NOT DEPLOYED).
 *
 * The hosted equivalent of the axum `server/src/ext_route.rs` route. The static
 * `_headers` file cannot carry a PER-EXTENSION CSP (it is one policy for all
 * paths), so the hosted build needs a Function to serve `/ext/<name>/` with a CSP
 * whose `connect-src` is reflected from the sanitized `o=` query params — exactly
 * as the local server does. Identical security model:
 *
 *   - The in-page host computes `GrantSet::connect_src()` (Rust) and encodes the
 *     granted origins as repeated `o=` params on the iframe `src`.
 *   - This Function RE-VALIDATES each origin (`sanitizeOrigin`) and reflects only
 *     the survivors into the response `connect-src`. A crafted URL cannot inject a
 *     header/directive or a wildcard; the floor is `connect-src 'none'`.
 *   - `sandbox="allow-scripts"` (set by the host on the iframe) forces an opaque
 *     origin, so the served document — though same-origin by URL — cannot reach
 *     the app's IndexedDB / localStorage. A response-header CSP (unlike a srcdoc
 *     `<meta>` CSP) is NOT inherited by the embedder, so a granted origin actually
 *     takes effect. See docs/EXTENSIONS.md §7.
 *
 * Deployment note: this is committed as the design of record. Wiring it into the
 * live Pages project (moving `functions/` into the deploy root, adjusting
 * `deploy-cloudflare.sh`) is a deliberate follow-up — the local axum route is the
 * primary, witnessed implementation; this keeps the hosted build's design pinned
 * and reviewable. The COOP/COEP headers are still supplied by `_headers` (they
 * apply to all paths including `/ext/*`).
 *
 * KEEP IN SYNC with `server/src/ext_route.rs` — the sanitizer grammar, the CSP
 * directive set, and the BOOTSTRAP_SHELL string must match byte-for-byte so the
 * local and hosted extension sandboxes behave identically.
 */

// The host-authored bootstrap shell. MUST match `ext_route::BOOTSTRAP_SHELL`.
const BOOTSTRAP_SHELL = `<!DOCTYPE html><html><head><meta charset="utf-8">
<style>
  :root { color-scheme: dark; }
  html,body { margin:0; padding:0; background:#12121a; color:#e0e0e8;
    font:13px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',system-ui,sans-serif; }
  #ext-root { padding:10px 12px; }
  a { color:#00d4aa; }
</style></head>
<body><div id="ext-root"></div>
<script>
  (function () {
    var EXT_ID = 'extension';
    var _handler = null;
    var _buffer = [];
    var _injected = false;
    function _deliver(message) {
      if (typeof _handler === 'function') {
        try { _handler(message); }
        catch (err) { console.error('[ext:' + EXT_ID + '] handler error', err); }
      } else { _buffer.push(message); }
    }
    globalThis.silent = {
      get extensionId() { return EXT_ID; },
      onHostMessage: function (fn) {
        _handler = fn;
        var pending = _buffer; _buffer = [];
        for (var i = 0; i < pending.length; i++) _deliver(pending[i]);
      },
      post: function (message) { parent.postMessage({ __silentExt: true, message: message }, '*'); },
      renderLocal: function (html) {
        var root = document.getElementById('ext-root');
        if (root) root.innerHTML = String(html);
      },
    };
    window.addEventListener('securitypolicyviolation', function (ev) {
      try {
        parent.postMessage({
          __silentExtCsp: true, extensionId: EXT_ID,
          violation: {
            directive: ev.effectiveDirective || ev.violatedDirective || '',
            blockedURI: ev.blockedURI || '',
            disposition: ev.disposition || 'enforce',
          },
        }, '*');
      } catch (e) { /* never let reporting throw */ }
    });
    function _injectSource(src) {
      if (_injected) return;
      _injected = true;
      var s = document.createElement('script');
      s.type = 'module';
      s.textContent = 'try {\\n' + String(src) + '\\n} catch (err) {' +
        " console.error('[ext] entrypoint threw', err);" +
        " var r = document.getElementById('ext-root');" +
        " if (r) r.textContent = 'Extension failed to run: ' + (err && err.message || err); }";
      document.body.appendChild(s);
    }
    window.addEventListener('message', function (ev) {
      var data = ev.data;
      if (!data) return;
      if (data.__silentExtInit) {
        if (typeof data.extensionId === 'string') EXT_ID = data.extensionId;
        if (typeof data.source === 'string') _injectSource(data.source);
        return;
      }
      if (data.__silentHostRender) {
        var root = document.getElementById('ext-root');
        if (root) root.innerHTML = String(data.html || '');
        return;
      }
      if (data.__silentHost) {
        var env = data.message;
        _deliver(env && env.message ? env.message : env);
      }
    });
    parent.postMessage({ __silentExtShellReady: true }, '*');
  })();
</script></body></html>`;

/**
 * Validate one origin. Mirrors `ext_route::sanitize_origin`. Accepts
 * `scheme://host[:port]` (scheme ∈ {http,https}; host is `[A-Za-z0-9.-]+`; port is
 * 1–5 digits). Rejects everything else (wildcards, quoted CSP keywords, paths,
 * userinfo, CRLF, semicolons, spaces). Returns the canonical origin or null.
 */
function sanitizeOrigin(raw: string): string | null {
  if (!raw || raw.length > 255) return null;
  const sep = raw.indexOf("://");
  if (sep < 0) return null;
  const scheme = raw.slice(0, sep);
  if (scheme !== "https" && scheme !== "http") return null;
  const rest = raw.slice(sep + 3); // host[:port]
  let host = rest;
  let port: string | null = null;
  const colon = rest.lastIndexOf(":");
  if (colon >= 0) {
    host = rest.slice(0, colon);
    port = rest.slice(colon + 1);
  }
  if (!host || !/^[A-Za-z0-9.-]+$/.test(host)) return null;
  if (port !== null && !/^[0-9]{1,5}$/.test(port)) return null;
  return raw;
}

/** Build `connect-src` from the `o=` params. Mirrors `connect_src_from_query`. */
function connectSrcFromQuery(url: URL): string {
  const MAX = 16;
  const out: string[] = [];
  for (const v of url.searchParams.getAll("o")) {
    const o = sanitizeOrigin(v);
    if (o && !out.includes(o)) {
      out.push(o);
      if (out.length >= MAX) break;
    }
  }
  return out.length ? out.join(" ") : "'none'";
}

/** The per-extension CSP. Mirrors `ext_route::extension_csp`. */
function extensionCsp(connectSrc: string): string {
  return (
    "default-src 'none'; " +
    "script-src 'unsafe-inline'; " +
    "style-src 'unsafe-inline'; " +
    "img-src data:; " +
    "font-src data:; " +
    "connect-src " + connectSrc
  );
}

// Cloudflare Pages Function entrypoint. Path is /ext/<name>[/]; only `o=` params
// matter (the <name> segment scopes the URL so each extension is a distinct doc).
export const onRequestGet: PagesFunction = async ({ request }) => {
  const url = new URL(request.url);
  const csp = extensionCsp(connectSrcFromQuery(url));
  return new Response(BOOTSTRAP_SHELL, {
    status: 200,
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Content-Security-Policy": csp,
      "Cache-Control": "no-store",
      // COOP/COEP come from _headers (apply to all paths). They are repeated here
      // defensively so the doc is embeddable in the isolated base page even if a
      // future _headers scoping change excludes /ext/*.
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    },
  });
};
