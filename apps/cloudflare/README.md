# apps/cloudflare — hosted-build Pages Function sketches (NOT DEPLOYED)

This directory holds the Cloudflare Pages **Function** equivalents of routes the
local axum `server/` serves dynamically but the static `_headers` file cannot
express. They are committed as the **design of record** for the hosted build and
are **not wired into the live deploy** — `deploy-cloudflare.sh` still ships the
static bundle + `_headers` only. The local axum routes are the primary, witnessed
implementations.

## `functions/ext/[[path]].ts` — per-extension document route (Task j2b)

The hosted equivalent of `server/src/ext_route.rs`. Serves `/ext/<name>/` with a
**per-extension response-header CSP** whose `connect-src` is reflected (after
re-validation) from the `o=` query params the in-page host derives from
`GrantSet::connect_src()`.

Why a Function and not `_headers`: `_headers` applies ONE policy to a path glob;
it cannot vary `connect-src` per extension per grant. A Function can. Without it,
the hosted build would 404 on `/ext/<name>/` and extensions would not run.

Security model (identical to the Rust route, see `docs/EXTENSIONS.md` §7):
- `sandbox="allow-scripts"` (no `allow-same-origin`) → opaque origin → the served
  document cannot read the app's IndexedDB / localStorage even though it is loaded
  from a same-origin URL.
- A response-header CSP is NOT inherited by the embedder (unlike the old srcdoc
  `<meta>` CSP that Chromium intersected with the base page — the j3 finding), so a
  granted origin actually takes effect; deny-by-default is `connect-src 'none'`.
- The sanitizer (`sanitizeOrigin`) re-validates every `o=` param as a strict
  `scheme://host[:port]` token, so a crafted URL cannot inject a header/directive,
  a wildcard, or a quoted CSP keyword.

**KEEP IN SYNC** with `server/src/ext_route.rs`: the sanitizer grammar, the CSP
directive set, and the `BOOTSTRAP_SHELL` string must match byte-for-byte so local
and hosted extension sandboxes behave identically. The sanitizer parity is checked
by hand against the Rust `#[test]` cases in that file.

## To actually deploy (deliberate follow-up, not done here)

1. Place `functions/` at the deploy root (Cloudflare Pages discovers `functions/`
   relative to the build output dir).
2. Adjust `deploy-cloudflare.sh` to copy `apps/cloudflare/functions/` into `dist/`.
3. Confirm `_headers` COOP/COEP still apply to `/ext/*` (Functions responses are
   merged with `_headers`), or rely on the COOP/COEP the Function sets itself.
4. Re-run the j2b witness against the deployed origin.
