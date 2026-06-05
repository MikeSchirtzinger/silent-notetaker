/**
 * test-network-probe — j2b acceptance fixture (NOT a shipping extension).
 *
 * Runs inside the per-extension sandboxed iframe served by /ext/<name>/. Its
 * manifest grants exactly one origin (https://httpbin.org). On boot it probes:
 *   - a GRANTED-origin fetch (httpbin) — must SUCCEED (the j2b fix),
 *   - an UNGRANTED-origin fetch (example.com) — must be BLOCKED by the
 *     per-extension connect-src,
 *   - host-storage reachability — must be BLOCKED (opaque origin isolation).
 * Results are posted to the host AND written into window.__probeResult on the
 * iframe so the witness harness can read them via the host's record-keeping and
 * the CSP-violation log.
 */
const result = { granted: 'pending', ungranted: 'pending', iso: {} };

function emit() {
  silent.post({
    type: 'render.panel',
    payload: { html: '<pre>' + JSON.stringify(result, null, 2) + '</pre>' },
  });
}

// Origin-isolation probes (must be blocked → SecurityError on an opaque origin).
try { result.iso.origin = String(window.origin); } catch (e) { result.iso.origin = 'throw:' + e.name; }
try { void window.localStorage; result.iso.localStorage = 'ACCESSIBLE'; }
catch (e) { result.iso.localStorage = 'blocked:' + e.name; }
try { window.indexedDB.open('SilentNotetaker'); result.iso.indexedDB = 'open-call-ok'; }
catch (e) { result.iso.indexedDB = 'blocked:' + e.name; }

// Granted-origin fetch — must SUCCEED now that grants ride on the response CSP.
fetch('https://httpbin.org/get', { mode: 'cors' })
  .then((r) => { result.granted = 'OK ' + r.status; emit(); })
  .catch((e) => { result.granted = 'BLOCKED ' + e.message; emit(); });

// Ungranted-origin fetch — must be BLOCKED by the per-extension connect-src.
fetch('https://example.com/', { mode: 'cors' })
  .then((r) => { result.ungranted = 'REACHED ' + r.status; emit(); })
  .catch((e) => { result.ungranted = 'BLOCKED ' + e.message; emit(); });

silent.onHostMessage(() => {});
emit();
