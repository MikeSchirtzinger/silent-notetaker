# Diarization: Over-Split Root Cause and Global Re-Clustering Design

**Status:** Design + ready-to-paste patch. QUALITY IMPROVEMENT IS UNPROVEN — Mike must validate with real audio in a browser. This document describes a hypothesis with an implementation, not a verified fix.

---

## 1. Root Cause: Why Online Leader Clustering Over-Splits

### The algorithm (as-read from index.html lines 1978–2057)

`SpeakerTracker.identify()` (line 2005) runs **online leader clustering** on every utterance. The decision rule is simple:

1. Compute cosine similarity between the new embedding and each stored centroid (line 2024).
2. If the best match exceeds `this.threshold` (currently **0.45**, line 1991), merge: update the centroid with a capped exponential moving average (`n = Math.min(sp.count, 50)`, line 2030) and re-normalize.
3. Otherwise, create a new speaker immediately and push it into `this.speakers` (line 2042).

The centroid update formula at line 2032–2034 is:

```
c[i] = (c[i] * n + emb[i]) / (n + 1)    # weighted mean (capped at 50 prior samples)
c[i] /= ||c||                             # L2 renormalize
```

Only the running centroid is stored per speaker. **Raw per-utterance embeddings are discarded after the centroid update.**

### Why this over-splits

**Problem 1 — Short-segment embedding noise.** `minSamples = 16000` (line 1992) means segments as short as exactly 1.0 s are embedded. TitaNet-small is trained on longer windows; 1-second segments produce high-variance embeddings. A single noisy outlier embedding from Speaker A can fall below 0.45 similarity to A's centroid and open a phantom `S3`.

**Problem 2 — Threshold sensitivity with no global view.** The threshold comment (lines 1987–1990) documents the tension directly:

> "0.35-0.45 → exact 6/6 (purity 1.0); 0.5+ over-splits"

This was tuned on one clean-audio 6-speaker session. In a real meeting — varying mic distance, background noise, room reverberation — the within-speaker cosine gap shrinks, pushing two embeddings from the same person below 0.45 and triggering a split. There is no recovery mechanism; once a new speaker is minted, its count grows and its centroid drifts independently.

**Problem 3 — Drift over long meetings.** The centroid cap at 50 means later embeddings are weighted equally with early ones after 50 utterances. A speaker whose voice characteristics shift (hoarse, excited, quieter) can drift their centroid far enough that their own next segment fails the threshold.

**Problem 4 — No global consistency check.** Online leader clustering makes irreversible greedy decisions. Two speakers minted 10 minutes apart may have centroids that, if compared at meeting end, are clearly the same person (cosine > 0.45). The algorithm never looks back.

**Problem 5 — DOM re-label is already in place.** `renameSpeaker()` (line 5806–5820) shows that changing a speaker label in the DOM is a matter of updating `speaker.name` in the tracker and querying all `.speaker-tag[data-speaker="${id}"]` elements. A global re-clustering pass can use the same mechanism to merge phantom speakers.

### What the code does NOT store

After the centroid update, the raw embedding `emb` (Float32Array(192)) is thrown away. Only the centroid and a `count` integer survive. This means a stop-time pass cannot re-cluster from original embeddings — it must work from the per-speaker centroids, or it must accumulate raw embeddings during the live pass first.

**This is a key design constraint. The patch below adds raw embedding accumulation as an additive change to `identify()`. Without it, the re-clustering can only work from centroids (much coarser), which limits quality.**

---

## 2. Design: Stop-Time Global Re-Clustering

### Principle: additive, never destructive

The live path (`SpeakerTracker.identify()`) stays exactly as-is. No threshold is changed. No existing logic is touched during recording. The new code:

1. Accumulates raw embeddings per utterance into a parallel array (a few KB of memory for a 2-hour meeting at 1 embedding per utterance).
2. Runs once, synchronously, when the user presses Stop, before the summary modal opens.
3. Produces a relabeling map (`oldId → canonicalId`) and applies it to the DOM.
4. Preserves any manual renames the user made.

### 2.1 Accumulating embeddings during the live pass

Add `this.utterances = []` to the constructor. In `identify()`, after computing `emb`, push a record before the centroid update decision:

```js
this.utterances.push({ emb: emb.slice(), assignedId: /* filled after the if/else below */ });
```

The `assignedId` is filled in at the end of identify: whichever speaker id is returned. The push must happen after the speaker id decision so we know which live id was assigned.

### 2.2 Agglomerative centroid-merge re-clustering

**Input:** `this.utterances` — an array of `{ emb: Float32Array(192), assignedId: string }`.

**Algorithm (single-linkage-like, but using group centroids — O(k²) per merge step where k is the number of live speakers, typically 2–12):**

1. Compute a "true centroid" per live speaker id by averaging all utterance embeddings assigned to that id. This is more robust than the online centroid because it is a simple unweighted mean over all utterances, not a capped EMA.
2. Build a k×k cosine similarity matrix over these true centroids.
3. Repeatedly find the pair (i, j) with the highest similarity. If `sim(i, j) >= RECLUSTER_THRESHOLD`, merge j into i (all utterances of j become i's; recompute i's centroid), remove j. Repeat until no pair exceeds the threshold.
4. This is O(k² × merges) — with k ≤ 12 speakers, this is trivially fast in JS.

**Choosing `RECLUSTER_THRESHOLD`:** The merge threshold should be **more permissive than the live threshold** (0.45) because:
- We are now comparing robust full-meeting centroids instead of noisy single-utterance embeddings.
- Two true centroids of the same speaker will be more similar than any single utterance.
- Start at **0.65**. Tune upward (0.70, 0.75) if merging real different speakers; tune downward (0.60, 0.55) if known over-splits survive.

This is a tunable constant, exposed as `SpeakerTracker.reclusterThreshold` so Mike can adjust it from the console without editing the file.

### 2.3 Stable relabeling

After clustering, produce a canonical id ordering. Sort surviving speaker groups by their earliest utterance timestamp (or by their original `nextId` sequence). Map them to `S1, S2, S3, …` in that order. Build a lookup table:

```
oldId → newCanonicalId
```

This is applied to:
- `this.speakers` array (update `id` fields in place, maintain `color`, `name`).
- All `.speaker-tag[data-speaker]` DOM elements (update `data-speaker` attribute and text content).
- All `[data-speaker-chip]` elements in the speakers bar.

### 2.4 Preserving manual renames

Before running the re-clustering, snapshot any `speaker.name` values that are non-empty (user has renamed them). After relabeling, re-apply those names to whatever canonical id the renamed speaker maps to:

```js
const renames = new Map(this.speakers.filter(s => s.name).map(s => [s.id, s.name]));
// ... do re-clustering, build oldToNew map ...
// After relabeling, for each old id with a rename:
for (const [oldId, name] of renames) {
  const newId = oldToNew.get(oldId) ?? oldId;
  const sp = this.speakers.find(s => s.id === newId);
  if (sp && !sp.name) sp.name = name; // don't clobber if the merged group already has a name
}
```

---

## 3. Patch: Ready-to-Paste JavaScript

### IMPORTANT: Two insertion points required.

**Insertion point A** — inside `SpeakerTracker.identify()`, adding raw embedding accumulation.  
**Insertion point B** — inside `SpeakerTracker` (after `displayName()`), adding the `globalRecluster()` method.  
**Insertion point C** — inside `App.stop()`, calling `globalRecluster()` before `showSummary()`.

### A: Accumulation in `identify()` — two small edits

**Edit A1:** Add `this.utterances = []` to the constructor.

Current constructor body (lines 1984–1997, search for exact anchor):
```js
  constructor() {
    this.speakers = []; // [{ id, name, color, centroid:Float32Array(192,L2), count }]
    this.colors = ['#00d4aa', '#ff6b6b', '#7b68ee', '#ffd700', '#ff8c42', '#4ecdc4', '#c678dd', '#61afef'];
```

Replace with:
```js
  constructor() {
    this.speakers = []; // [{ id, name, color, centroid:Float32Array(192,L2), count }]
    this.utterances = []; // [{ emb: Float32Array(192), assignedId: string }] — for stop-time recluster
    this.colors = ['#00d4aa', '#ff6b6b', '#7b68ee', '#ffd700', '#ff8c42', '#4ecdc4', '#c678dd', '#61afef'];
```

**Edit A2:** Record the embedding at the return point of `identify()`. Replace the two `return` statements that go through the live path (lines 2037 and 2044) so the embedding is stored with its assigned id.

Current code (search anchor — lines 2028–2044):
```js
    if (best >= 0 && bestSim >= this.threshold) {
      const sp = this.speakers[best];
      const c = sp.centroid, n = Math.min(sp.count, 50);
      let nn = 0;
      for (let i = 0; i < c.length; i++) { c[i] = (c[i] * n + emb[i]) / (n + 1); nn += c[i] * c[i]; }
      nn = Math.sqrt(nn) || 1;
      for (let i = 0; i < c.length; i++) c[i] /= nn; // keep centroid L2-normalized
      sp.count += 1;
      this.lastSpeakerId = sp.id;
      return { id: sp.id, name: sp.name, color: sp.color, isNew: false };
    }

    const id = `S${this.nextId++}`;
    const color = this.colors[this.speakers.length % this.colors.length];
    this.speakers.push({ id, name: '', color, centroid: emb.slice(), count: 1 });
    this.lastSpeakerId = id;
    return { id, name: '', color, isNew: true };
```

Replace with:
```js
    if (best >= 0 && bestSim >= this.threshold) {
      const sp = this.speakers[best];
      const c = sp.centroid, n = Math.min(sp.count, 50);
      let nn = 0;
      for (let i = 0; i < c.length; i++) { c[i] = (c[i] * n + emb[i]) / (n + 1); nn += c[i] * c[i]; }
      nn = Math.sqrt(nn) || 1;
      for (let i = 0; i < c.length; i++) c[i] /= nn; // keep centroid L2-normalized
      sp.count += 1;
      this.lastSpeakerId = sp.id;
      this.utterances.push({ emb: emb.slice(), assignedId: sp.id }); // accumulate for stop-time recluster
      return { id: sp.id, name: sp.name, color: sp.color, isNew: false };
    }

    const id = `S${this.nextId++}`;
    const color = this.colors[this.speakers.length % this.colors.length];
    this.speakers.push({ id, name: '', color, centroid: emb.slice(), count: 1 });
    this.lastSpeakerId = id;
    this.utterances.push({ emb: emb.slice(), assignedId: id }); // accumulate for stop-time recluster
    return { id, name: '', color, isNew: true };
```

### B: The `globalRecluster()` method — insert after `displayName()` (after line 2057, before the closing `}` of the class)

Search anchor — the end of `SpeakerTracker`:
```js
  displayName(speakerId) {
    const speaker = this.speakers.find(s => s.id === speakerId);
    if (!speaker) return speakerId;
    return speaker.name || speaker.id;
  }
}
```

Replace with:
```js
  displayName(speakerId) {
    const speaker = this.speakers.find(s => s.id === speakerId);
    if (!speaker) return speakerId;
    return speaker.name || speaker.id;
  }

  /**
   * Stop-time global re-clustering pass.
   *
   * ADDITIVE — the live path is untouched. This runs once when recording stops,
   * over the accumulated per-utterance embeddings, to merge phantom speakers that
   * online leader clustering created due to noisy short-segment embeddings.
   *
   * UNPROVEN — quality improvement is a hypothesis. Mike must validate with real
   * multi-speaker audio in a browser. See docs/DIARIZATION.md §4 for test procedure.
   *
   * @param {number} [threshold=0.65] Cosine similarity above which two speaker
   *   clusters are merged. Higher = fewer merges. Lower = more aggressive merging.
   *   Tune from the browser console: app.tm.speakerTracker.reclusterThreshold = 0.60
   */
  async globalRecluster(threshold) {
    const th = threshold ?? this.reclusterThreshold ?? 0.65;

    if (this.utterances.length < 2 || this.speakers.length < 2) return; // nothing to do

    // Step 1: Build true (unweighted) centroids from raw utterance embeddings.
    // These are more robust than the online EMA centroids because they treat every
    // utterance equally and are not limited by the 50-sample cap.
    const dim = this.utterances[0].emb.length; // 192
    const centroidMap = new Map(); // id -> Float64Array(192) (use f64 for accumulation precision)
    const countMap = new Map();    // id -> number
    for (const { emb, assignedId } of this.utterances) {
      if (!centroidMap.has(assignedId)) {
        centroidMap.set(assignedId, new Float64Array(dim));
        countMap.set(assignedId, 0);
      }
      const c = centroidMap.get(assignedId);
      for (let i = 0; i < dim; i++) c[i] += emb[i];
      countMap.set(assignedId, countMap.get(assignedId) + 1);
    }

    // Normalize each true centroid to unit length.
    const trueCentroids = new Map();
    for (const [id, c] of centroidMap) {
      const n = countMap.get(id);
      let norm = 0;
      for (let i = 0; i < dim; i++) { c[i] /= n; norm += c[i] * c[i]; }
      norm = Math.sqrt(norm) || 1;
      const v = new Float32Array(dim);
      for (let i = 0; i < dim; i++) v[i] = c[i] / norm;
      trueCentroids.set(id, v);
    }

    // Only work with speaker ids we actually know about (ignore any stale ids).
    let activeIds = this.speakers.map(s => s.id).filter(id => trueCentroids.has(id));
    if (activeIds.length < 2) return;

    // Snapshot user renames before we touch anything.
    const renames = new Map(this.speakers.filter(s => s.name).map(s => [s.id, s.name]));

    // Step 2: Agglomerative centroid merge.
    // Build a map from survivor id -> set of merged original ids.
    const mergedInto = new Map(activeIds.map(id => [id, id])); // id -> canonical id (initially self)

    // find(id) resolves the current canonical representative.
    const find = (id) => {
      let r = mergedInto.get(id);
      while (r !== mergedInto.get(r)) { r = mergedInto.get(r); }
      return r;
    };

    let merged = true;
    while (merged) {
      merged = false;
      let bestSim = -1, bestI = null, bestJ = null;

      // Find the most similar pair of surviving clusters.
      for (let a = 0; a < activeIds.length; a++) {
        const ca = trueCentroids.get(find(activeIds[a]));
        if (!ca) continue;
        for (let b = a + 1; b < activeIds.length; b++) {
          const cb = trueCentroids.get(find(activeIds[b]));
          if (!cb) continue;
          if (find(activeIds[a]) === find(activeIds[b])) continue; // already merged
          let sim = 0;
          for (let i = 0; i < dim; i++) sim += ca[i] * cb[i];
          if (sim > bestSim) { bestSim = sim; bestI = find(activeIds[a]); bestJ = find(activeIds[b]); }
        }
      }

      if (bestSim >= th && bestI !== null && bestJ !== null) {
        // Merge bestJ into bestI (keep the one that appeared first by id sort).
        // Recompute bestI's true centroid from all utterances now assigned to it.
        for (const u of this.utterances) {
          if (find(u.assignedId) === bestJ) u.assignedId = bestI; // redirect utterances
        }
        mergedInto.set(bestJ, bestI);

        // Recompute bestI's true centroid.
        const newC = new Float64Array(dim);
        let cnt = 0;
        for (const { emb, assignedId } of this.utterances) {
          if (find(assignedId) === bestI) { for (let i = 0; i < dim; i++) newC[i] += emb[i]; cnt++; }
        }
        let norm = 0;
        for (let i = 0; i < dim; i++) { newC[i] /= cnt || 1; norm += newC[i] * newC[i]; }
        norm = Math.sqrt(norm) || 1;
        const v = new Float32Array(dim);
        for (let i = 0; i < dim; i++) v[i] = newC[i] / norm;
        trueCentroids.set(bestI, v);
        trueCentroids.delete(bestJ);
        merged = true;
      }
    }

    // Step 3: Build the old->new relabeling map.
    // Surviving clusters = ids that are still their own canonical representative.
    const survivors = [...new Set(activeIds.map(id => find(id)))];

    // Sort by the original numeric suffix so S1 stays S1 (stable ordering).
    survivors.sort((a, b) => {
      const na = parseInt(a.replace(/\D/g, ''), 10) || 0;
      const nb = parseInt(b.replace(/\D/g, ''), 10) || 0;
      return na - nb;
    });

    const oldToNew = new Map();
    survivors.forEach((survivorId, idx) => {
      const newId = `S${idx + 1}`;
      // Every original speaker that merged into this survivor maps to newId.
      for (const origId of activeIds) {
        if (find(origId) === survivorId) oldToNew.set(origId, newId);
      }
    });

    // Bail out if nothing changed (no merges happened, ids are already S1..Sn).
    const anyChange = [...oldToNew.entries()].some(([k, v]) => k !== v);
    if (!anyChange) {
      console.log('[speaker] globalRecluster: no merges needed at threshold', th);
      return;
    }

    console.log('[speaker] globalRecluster: merging', this.speakers.length, '→', survivors.length,
      'speakers at threshold', th, '| map:', [...oldToNew.entries()].map(([k,v]) => `${k}→${v}`).join(', '));

    // Step 4: Apply relabeling.
    // 4a: Rebuild this.speakers with new ids, reusing color/centroid from the survivor,
    //     re-applying any user rename.
    const newSpeakers = survivors.map((survivorId, idx) => {
      const newId = `S${idx + 1}`;
      const orig = this.speakers.find(s => s.id === survivorId);
      // Find the best user rename for this merged group (prefer the earliest renamed id).
      let name = '';
      for (const [oldId, renamedName] of renames) {
        if (oldToNew.get(oldId) === newId) { name = renamedName; break; }
      }
      return {
        id: newId,
        name,
        color: orig ? orig.color : this.colors[idx % this.colors.length],
        centroid: trueCentroids.get(survivorId) ?? (orig ? orig.centroid : new Float32Array(dim)),
        count: orig ? orig.count : 1,
      };
    });
    this.speakers = newSpeakers;

    // 4b: Update the DOM — all .speaker-tag and [data-speaker-chip] elements.
    // We must do this in two passes to avoid id collisions mid-update:
    // First stamp a temp attribute, then write the final id.
    document.querySelectorAll('.speaker-tag[data-speaker]').forEach(el => {
      const newId = oldToNew.get(el.dataset.speaker);
      if (newId) {
        const sp = this.speakers.find(s => s.id === newId);
        el.dataset.speaker = newId;
        if (sp) el.style.color = sp.color;
        // Preserve the display name: if the element shows a raw Sx id (no user rename),
        // update it to the new id; if it already shows a human name, leave it.
        const currentText = el.textContent.trim();
        if (!currentText || /^S\d+$/.test(currentText)) {
          el.textContent = sp ? (sp.name || sp.id) : newId;
        } else if (sp && sp.name) {
          el.textContent = sp.name;
        }
      }
    });

    // 4c: Refresh the speakers bar.
    if (typeof renderSpeakerTags === 'function') renderSpeakerTags();
  }
}
```

### C: Call site in `App.stop()` — insert before `this.showSummary()`

Search anchor (lines 3859–3861, inside `async stop()`):
```js
    this.handleStatus('Recording stopped', null);

    this.showSummary();
```

Replace with:
```js
    this.handleStatus('Recording stopped', null);

    // Stop-time global re-clustering: merge phantom speakers created by online
    // leader clustering. ADDITIVE — live path is untouched. UNPROVEN until
    // tested with real multi-speaker audio. See docs/DIARIZATION.md.
    if (this.tm && this.tm.speakerTracker && this.tm.speakerTracker.speakers.length > 1) {
      try {
        await this.tm.speakerTracker.globalRecluster();
      } catch (e) {
        console.warn('[speaker] globalRecluster failed (non-fatal):', e);
      }
    }

    this.showSummary();
```

### Memory impact

At 1 embedding per utterance, 192 Float32 values = 768 bytes per utterance. A 2-hour meeting at one utterance every 10 seconds = ~720 utterances = ~540 KB. Acceptable.

### Console tuning knobs (no code edit needed)

```js
// Check how many speakers were detected live
app.tm.speakerTracker.speakers.length

// Try a more aggressive merge threshold before stopping
app.tm.speakerTracker.reclusterThreshold = 0.60

// Manually trigger after stopping (if you forgot to set threshold first)
await app.tm.speakerTracker.globalRecluster(0.60)

// Inspect the accumulated utterance embedding count
app.tm.speakerTracker.utterances.length
```

---

## 4. NEEDS-BROWSER-TEST: Test Procedure

**Cannot be validated in this environment. No audio, no GPU, no mic.**

### Test A: 2-voice check (minimum viable)

**Setup:** Two distinct speakers, 5–10 minutes, one mic, conversational exchange.

**Expected live behavior (before Stop):** App labels utterances `S1` and `S2`. Some utterances from the same person may appear as `S3`, `S4` due to over-splitting.

**What to check after Stop:**
1. Open browser console (`F12 → Console`).
2. Look for the log line: `[speaker] globalRecluster: merging X → Y speakers at threshold 0.65`.
3. Count speakers in the bar. "Better" = fewer phantom speakers; ideally 2 for a 2-person conversation.
4. Scroll the transcript. Check that speaker tags on consecutive same-person lines are consistent (same `Sx` label throughout).

**Pass criterion (hypothesis, not guaranteed):** The post-Stop speaker count is closer to 2 than the live count. Manual inspection of the transcript confirms that obvious same-voice runs share a label.

**Failure modes to watch for:**
- `globalRecluster` merges two real different speakers (under-threshold). Console log will show the merge; inspect which ids were collapsed.
- `globalRecluster` throws an exception (check console; the try/catch in `App.stop()` makes it non-fatal).
- Speaker bar disappears or shows wrong colors after re-clustering (DOM update bug).
- Utterance count in `app.tm.speakerTracker.utterances.length` is 0 — means Edit A2 was not applied, accumulation is not running.

### Test B: 6-voice check (stress test)

**Setup:** Six distinct speakers in a round-table or recorded multi-speaker call, 20–30 minutes.

**Expected live behavior:** App likely produces 8–15 live speaker ids due to over-splitting.

**What to check after Stop:**
1. Console log: note the `merging X → Y` numbers.
2. Speakers bar: count surviving speakers. "Better" = closer to 6 than the live count.
3. If recluster collapses too aggressively (e.g. 6 → 3), raise `reclusterThreshold` to 0.70 or 0.75 and re-run manually: `await app.tm.speakerTracker.globalRecluster(0.72)`.
4. If recluster does nothing (live count unchanged), lower threshold to 0.60 and re-run.

**Pass criterion (hypothesis):** Speaker count post-Stop is within ±1 of the true speaker count, and the dominant speaker across a 5-minute block carries a consistent label.

### Honest assessment

This is an **untested hypothesis** derived from standard speaker diarization literature (agglomerative hierarchical clustering is the conventional post-processing step after online segmentation). The implementation is consistent with the existing code style and uses no external dependencies. However:

- The threshold value 0.65 is a starting guess. It has NOT been calibrated against any audio in this codebase.
- The centroid-merge approach loses information compared to a full re-embedding pass (re-running TitaNet on the stored PCM), but raw PCM is not stored, so this is the best available option without a more invasive change.
- Agglomerative single-pass is sensitive to the first merge decision. If two very similar phantom clusters merge early, the merged centroid may be close enough to a third (real, different) speaker to trigger a false merge. The 0.65 threshold is designed to be conservative.
- The DOM relabeling logic has not been exercised on a live browser. Bugs in the `find()` union-find, in the ordering of DOM updates, or in `renderSpeakerTags()` could produce cosmetic glitches.

**Mike should treat this as a first hypothesis, measure it, and adjust the threshold or the merge strategy based on what he observes.**
