//! Per-extension document route — the network-grant keystone (PRD R7, Task j2b).
//!
//! # Why this route exists
//!
//! Extensions run in a null-origin sandboxed iframe (`sandbox="allow-scripts"`,
//! deliberately *without* `allow-same-origin`). The earlier design booted that
//! iframe from a `srcdoc` string carrying a `<meta>` CSP. The j3 finding proved
//! that does not work for *grants*: a `srcdoc` (and `blob:`) iframe **inherits the
//! embedder's CSP by intersection** in Chromium — a child may only *tighten*,
//! never *widen*, the base page `connect-src`. So a granted network origin (which
//! is broader than the base page policy, by design) stays blocked. Deny-by-default
//! worked; grants were inert.
//!
//! The fix (witnessed in `/Users/mike/dev/snt-spikes/j2b-grants/`): serve each
//! extension from a **distinct same-origin URL** whose **response-header** CSP
//! carries only that extension's policy. A response-header CSP is NOT inherited
//! from the embedder — it is the document's own policy — so a granted origin in
//! the response `connect-src` actually takes effect.
//!
//! # Isolation is still real (the key result)
//!
//! `sandbox="allow-scripts"` forces an **opaque origin regardless of the URL the
//! iframe loads from**. Loading from a real same-origin route therefore does NOT
//! grant the iframe same-origin access: the spike witnessed `window.origin ===
//! "null"` and `SecurityError` on `localStorage`/`indexedDB` for the served
//! document. The extension cannot reach the app's `SilentNotetaker` IndexedDB or
//! its localStorage. We keep `allow-scripts`, keep isolation, and need NO distinct
//! port and NO `credentialless` iframe.
//!
//! # How the per-extension CSP is parameterized
//!
//! The static server has no IndexedDB and cannot read grant sets (they live in the
//! browser). The in-page host (trusted) computes `GrantSet::connect_src()` in Rust
//! and passes the granted origins to this route as a repeated `o=<origin>` query
//! parameter. This route **independently re-validates** each origin as a strict
//! `scheme://host[:port]` token (see [`sanitize_origin`]) and reflects only the
//! survivors into the response `connect-src`. A crafted URL therefore cannot inject
//! a header, a `'self'`, a wildcard, or a CRLF — the worst an attacker can do is
//! widen *their own* sandbox's egress to another well-formed origin, which is no
//! worse than declaring it in the manifest. No origins ⇒ `connect-src 'none'`.
//!
//! The document body is a FIXED host-authored bootstrap shell (no extension code).
//! It announces readiness, then injects the entrypoint **source** the host posts
//! (the host fetched it same-origin; an opaque-origin doc still cannot `import()`
//! cross-origin, so injection as an inline `<script type=module>` — allowed by
//! `script-src 'unsafe-inline'` — is the delivery path, witnessed in the spike).

use axum::{
    extract::RawQuery,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

/// Validate one query-supplied origin and return its canonical form, or `None`.
///
/// Accepted: `scheme://host[:port]` where
/// - `scheme` is `https` or `http` (the only schemes a manifest network grant can
///   carry — the SDK manifest validator already enforces this upstream; we
///   re-check so a hand-crafted iframe URL cannot smuggle e.g. `javascript:`),
/// - `host` is a non-empty run of `[A-Za-z0-9.-]` (DNS-ish; no wildcards, no
///   userinfo `@`, no path `/`, no whitespace),
/// - optional `:port` is 1–5 digits.
///
/// Rejected (returns `None`): anything with a space, `'`, `;`, `,`, control char,
/// CR/LF, a path, a query, a wildcard `*`, `'self'`, `'none'`, `data:`/`blob:`
/// (those are scheme-less here), or an empty/oversized string. This is the
/// header-injection and policy-widening guard: only well-formed network origins
/// survive, and they go into `connect-src` verbatim.
#[must_use]
pub fn sanitize_origin(raw: &str) -> Option<String> {
    // Bound the length so a pathological query can't bloat the header.
    if raw.is_empty() || raw.len() > 255 {
        return None;
    }

    // Split scheme://rest. The ONLY `/` allowed is the `://` separator; anything
    // after `rest` is host[:port] and is validated by an allow-list (alphanumeric
    // + `.` + `-` for the host, digits for the port). That allow-list is the real
    // header-injection guard: a space, `;`, `'`, CR/LF, `*`, `@`, path `/`, or any
    // control char simply is not in the host/port grammar, so it cannot survive.
    let (scheme, rest) = raw.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    // `rest` must be host[:port] only — split a single trailing :port if present.
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return None;
    }
    if let Some(p) = port {
        if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(raw.to_string())
}

/// Build the `connect-src` directive value from the raw query string's `o=`
/// params. Returns `'none'` when nothing valid was supplied (deny by default).
///
/// Origins are de-duplicated, kept in first-seen order, and capped (a single
/// extension realistically grants a handful of origins; the cap bounds the header
/// against a hostile query). Each must pass [`sanitize_origin`].
fn connect_src_from_query(raw_query: Option<&str>) -> String {
    const MAX_ORIGINS: usize = 16;
    let mut origins: Vec<String> = Vec::new();
    if let Some(q) = raw_query {
        for pair in q.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            if k != "o" {
                continue;
            }
            // Percent-decode the value (origins contain `://` and `:` which are
            // commonly encoded). A decode failure ⇒ skip (deny that entry).
            let decoded = percent_decode(v);
            if let Some(origin) = sanitize_origin(&decoded) {
                if !origins.contains(&origin) {
                    origins.push(origin);
                    if origins.len() >= MAX_ORIGINS {
                        break;
                    }
                }
            }
        }
    }
    if origins.is_empty() {
        "'none'".to_string()
    } else {
        origins.join(" ")
    }
}

/// Minimal, allocation-light percent-decoder for query values. Invalid escapes
/// are left literal (they will then fail [`sanitize_origin`] and be dropped, so a
/// malformed escape can never widen the policy).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The full per-extension CSP value for the served document.
///
/// `default-src 'none'` is the floor; `script-src`/`style-src` allow the inlined
/// bootstrap + the host-posted module + the extension's own inline styles;
/// `img-src`/`font-src` are pinned to `data:` so an extension cannot beacon via an
/// `<img>`; `connect-src` is EXACTLY the granted origins (or `'none'`). There is no
/// `frame-src`/`frame-ancestors` here beyond the floor — the doc neither frames nor
/// is meant to be reframed elsewhere (the base page `frame-src 'self'` governs
/// who may embed it).
fn extension_csp(connect_src: &str) -> String {
    format!(
        "default-src 'none'; \
         script-src 'unsafe-inline'; \
         style-src 'unsafe-inline'; \
         img-src data:; \
         font-src data:; \
         connect-src {connect_src}"
    )
}

/// The host-authored bootstrap shell served at `/ext/<name>/`.
///
/// This is the SAME bootstrap contract `extension-host.js` speaks, moved into a
/// real document so the response-header CSP applies. It contains NO extension code
/// and NO host data: it wires `globalThis.silent`, buffers host messages until the
/// extension registers a handler, reports its OWN CSP violations to the parent
/// (R7), and injects the entrypoint module SOURCE the host posts once
/// (`__silentExtSource`). Origin isolation (opaque origin) means it cannot reach
/// the host page; the only channel is `postMessage`.
const BOOTSTRAP_SHELL: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<style>
  :root { color-scheme: dark; }
  html,body { margin:0; padding:0; background:#12121a; color:#e0e0e8;
    font:13px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',system-ui,sans-serif; }
  #ext-root { padding:10px 12px; }
  a { color:#00d4aa; }
</style></head>
<body><div id="ext-root"></div>
<script>
  // Bootstrap (classic script, runs synchronously). Defines globalThis.silent and
  // the single host channel, BUFFERS host messages until the extension registers
  // its handler, and injects the entrypoint module source the host posts exactly
  // once. The opaque origin means no host globals are reachable; postMessage is
  // the only channel. EXT_ID is learned from the host's first __silentExtInit.
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
    // R7: report this sandbox's OWN CSP violations to the host so a blocked,
    // undeclared fetch is LOGGED (not silently swallowed).
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
      // Wrap so an entrypoint throw renders a message instead of a blank panel.
      s.textContent = 'try {\n' + String(src) + '\n} catch (err) {' +
        " console.error('[ext] entrypoint threw', err);" +
        " var r = document.getElementById('ext-root');" +
        " if (r) r.textContent = 'Extension failed to run: ' + (err && err.message || err); }";
      document.body.appendChild(s);
    }
    window.addEventListener('message', function (ev) {
      var data = ev.data;
      if (!data) return;
      // First message from the host: our id + the entrypoint module source.
      if (data.__silentExtInit) {
        if (typeof data.extensionId === 'string') EXT_ID = data.extensionId;
        if (typeof data.source === 'string') _injectSource(data.source);
        return;
      }
      // Host-echoed panel HTML (validated against the panel grant): render it
      // into OUR OWN sandbox document.
      if (data.__silentHostRender) {
        var root = document.getElementById('ext-root');
        if (root) root.innerHTML = String(data.html || '');
        return;
      }
      // A versioned host->extension envelope: deliver the inner message body.
      if (data.__silentHost) {
        var env = data.message;
        _deliver(env && env.message ? env.message : env);
      }
    });
    // Announce readiness so the host posts __silentExtInit (id + source).
    parent.postMessage({ __silentExtShellReady: true }, '*');
  })();
</script></body></html>"#;

/// Serve the per-extension bootstrap document with its own response-header CSP.
///
/// Route: `GET /ext/{name}/?o=<origin>&o=<origin>…`. The `name` segment is purely
/// cosmetic for the URL (it scopes the document path so each extension has a
/// distinct same-origin URL); the real policy comes from the sanitized `o=` params
/// reflected into `connect-src`. COOP/COEP are NOT set here — the surrounding
/// router layers them on every response (the doc must carry require-corp so it can
/// be embedded in the cross-origin-isolated base page), and the sandbox forces the
/// opaque origin that keeps it isolated.
pub async fn serve_extension_doc(RawQuery(query): RawQuery) -> Response {
    let connect_src = connect_src_from_query(query.as_deref());
    let csp = extension_csp(&connect_src);

    let mut resp = (StatusCode::OK, BOOTSTRAP_SHELL).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // The per-extension CSP. `from_str` cannot fail for a value built only from
    // sanitized origins + static directive text, but if it somehow did we fall
    // back to the strictest possible policy rather than serving uncontrolled.
    let csp_value = HeaderValue::from_str(&csp)
        .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'; connect-src 'none'"));
    headers.insert(header::CONTENT_SECURITY_POLICY, csp_value);
    // Never cache a per-extension doc (the CSP varies by grant).
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_https_origin() {
        assert_eq!(
            sanitize_origin("https://api.notion.com").as_deref(),
            Some("https://api.notion.com")
        );
    }

    #[test]
    fn accepts_http_and_port() {
        assert_eq!(
            sanitize_origin("http://localhost:8765").as_deref(),
            Some("http://localhost:8765")
        );
    }

    #[test]
    fn rejects_wildcard_and_quoted_keywords() {
        // A wildcard host or a CSP keyword would WIDEN the policy beyond a single
        // origin — both must be dropped.
        assert!(sanitize_origin("https://*.evil.com").is_none());
        assert!(sanitize_origin("'self'").is_none());
        assert!(sanitize_origin("'none'").is_none());
        assert!(sanitize_origin("'unsafe-inline'").is_none());
    }

    #[test]
    fn rejects_header_and_directive_injection() {
        // CRLF / semicolon / space would inject a header or a second directive.
        assert!(sanitize_origin("https://a.com;script-src *").is_none());
        assert!(sanitize_origin("https://a.com connect-src *").is_none());
        assert!(sanitize_origin("https://a.com\r\nSet-Cookie: x=y").is_none());
        assert!(sanitize_origin("https://a.com\nfoo").is_none());
    }

    #[test]
    fn rejects_non_network_schemes_and_paths() {
        assert!(sanitize_origin("javascript:alert(1)").is_none());
        assert!(sanitize_origin("data:text/html,x").is_none());
        assert!(sanitize_origin("file:///etc/passwd").is_none());
        // A path is not part of an origin.
        assert!(sanitize_origin("https://a.com/path").is_none());
        // Userinfo is not allowed.
        assert!(sanitize_origin("https://user@a.com").is_none());
    }

    #[test]
    fn rejects_empty_and_oversized() {
        assert!(sanitize_origin("").is_none());
        let long = format!("https://{}.com", "a".repeat(300));
        assert!(sanitize_origin(&long).is_none());
    }

    #[test]
    fn empty_query_denies_all_network() {
        assert_eq!(connect_src_from_query(None), "'none'");
        assert_eq!(connect_src_from_query(Some("")), "'none'");
        assert_eq!(connect_src_from_query(Some("foo=bar")), "'none'");
    }

    #[test]
    fn single_granted_origin_reflected() {
        assert_eq!(
            connect_src_from_query(Some("o=https%3A%2F%2Fapi.notion.com")),
            "https://api.notion.com"
        );
    }

    #[test]
    fn multiple_origins_deduped_in_order() {
        let q = "o=https%3A%2F%2Fb.com&o=https%3A%2F%2Fa.com&o=https%3A%2F%2Fb.com";
        assert_eq!(
            connect_src_from_query(Some(q)),
            "https://b.com https://a.com"
        );
    }

    #[test]
    fn malicious_origin_dropped_others_kept() {
        // A crafted param trying to inject a directive is dropped; the legit one
        // survives. The header can never carry the injection.
        let q = "o=https%3A%2F%2Fgood.com&o=https%3A%2F%2Fx.com%3Bscript-src%20*";
        assert_eq!(connect_src_from_query(Some(q)), "https://good.com");
    }

    #[test]
    fn csp_floor_is_default_none() {
        let csp = extension_csp("'none'");
        assert!(csp.starts_with("default-src 'none'"));
        assert!(csp.contains("connect-src 'none'"));
        // No script/style 'unsafe-eval', no wildcard.
        assert!(!csp.contains("unsafe-eval"));
        assert!(!csp.contains('*'));
    }

    #[test]
    fn csp_reflects_granted_origin_in_connect_src_only() {
        let csp = extension_csp("https://api.notion.com");
        assert!(csp.contains("connect-src https://api.notion.com"));
        // The grant must NOT leak into script-src / img-src etc.
        assert!(csp.contains("script-src 'unsafe-inline'"));
        assert!(!csp.contains("script-src 'unsafe-inline' https://api.notion.com"));
        assert!(csp.contains("img-src data:"));
    }
}
