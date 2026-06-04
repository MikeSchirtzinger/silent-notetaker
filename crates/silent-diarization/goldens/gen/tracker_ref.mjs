// Reference SpeakerTracker — a faithful, DOM-free port of the JS algorithm in
// index.html (class SpeakerTracker, ~lines 1985–2093; rename/merge functions
// ~6250–6301). This is the BEHAVIOR CONTRACT the Rust port must reproduce
// exactly. The recluster is ported from docs/DIARIZATION.md §2 (the centroid
// logic only — no DOM).
//
// Run: node tracker_ref.mjs  → writes ../tracker/*.json and ../recluster/*.json
//
// Embeddings here are deterministic synthetic vectors (a tiny seeded RNG), NOT
// real audio: the point is to pin the CLUSTERING math, which is audio-agnostic.
// Real-audio cosine 1.000000 is proven separately by the TitaNet golden test.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const DIM = 192;

// ---- deterministic RNG (mulberry32) so fixtures are reproducible ----
function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function l2norm(v) {
  let n = 0;
  for (let i = 0; i < v.length; i++) n += v[i] * v[i];
  n = Math.sqrt(n) || 1;
  const out = new Float64Array(v.length);
  for (let i = 0; i < v.length; i++) out[i] = v[i] / n;
  return out;
}

// A unit "speaker prototype" vector for a given id (deterministic).
function prototype(seed) {
  const rng = mulberry32(seed);
  const v = new Float64Array(DIM);
  for (let i = 0; i < DIM; i++) v[i] = rng() * 2 - 1;
  return l2norm(v);
}

// An utterance embedding = prototype + small jitter, re-normalized. `jitter`
// controls within-speaker spread. Note: in 192-dim, random jitter spreads
// cosine fast (jitter 0.2 already drops cos-to-proto below 0.45), so keep it
// small for "clean" utterances.
function utteranceEmb(proto, jitterSeed, jitter) {
  const rng = mulberry32(jitterSeed);
  const v = new Float64Array(DIM);
  for (let i = 0; i < DIM; i++) v[i] = proto[i] + (rng() * 2 - 1) * jitter;
  return l2norm(v);
}

// Over-split jitter level. In 192-dim, jitter 0.12 gives same-speaker utterances
// that the DEFAULT 0.45 live threshold clusters cleanly (2 speakers, no split) —
// but a TOO-HIGH live threshold (0.55, the DIARIZATION.md "0.5+ over-splits"
// regime) shatters each speaker into one big cluster + several phantoms whose
// robust TRUE centroids still sit ~0.69 cosine to the speaker's main cluster.
// That is the window where the stop-time recluster (on true centroids) measurably
// repairs the over-split. The live threshold is set per-fixture below. Diagnosed
// empirically; see the diag scripts referenced in the F1 report.
const MESSY_JITTER = 0.12;
// Live threshold that triggers the over-split (DIARIZATION.md Problem 2).
const OVERSPLIT_THRESHOLD = 0.55;

// =====================================================================
// SpeakerTracker — DOM-free port of index.html (exact algorithm).
// =====================================================================
class SpeakerTracker {
  constructor() {
    this.speakers = []; // [{ id, name, color, centroid:Float64Array(192,L2), count }]
    this.utterances = []; // [{ emb, assignedId }] — for stop-time recluster
    this.colors = [
      "#00d4aa", "#ff6b6b", "#7b68ee", "#ffd700",
      "#ff8c42", "#4ecdc4", "#c678dd", "#61afef",
    ];
    this.threshold = 0.45;
    this.reclusterThreshold = 0.65;
    this.nextId = 1;
    this.lastSpeakerId = null;
  }

  cosine(a, b) {
    let d = 0;
    for (let i = 0; i < a.length; i++) d += a[i] * b[i];
    return d;
  }

  // Synchronous identify (the embedder is already-run; we feed embeddings).
  // Mirrors index.html identify() minus the async embed() and the minSamples
  // short-segment branch (the fixtures feed full embeddings directly).
  identify(emb) {
    let best = -1,
      bestSim = -1;
    for (let i = 0; i < this.speakers.length; i++) {
      const sim = this.cosine(emb, this.speakers[i].centroid);
      if (sim > bestSim) {
        bestSim = sim;
        best = i;
      }
    }

    if (best >= 0 && bestSim >= this.threshold) {
      const sp = this.speakers[best];
      const c = sp.centroid,
        n = Math.min(sp.count, 50);
      let nn = 0;
      for (let i = 0; i < c.length; i++) {
        c[i] = (c[i] * n + emb[i]) / (n + 1);
        nn += c[i] * c[i];
      }
      nn = Math.sqrt(nn) || 1;
      for (let i = 0; i < c.length; i++) c[i] /= nn;
      sp.count += 1;
      this.lastSpeakerId = sp.id;
      this.utterances.push({ emb: Float64Array.from(emb), assignedId: sp.id });
      return { id: sp.id, name: sp.name, color: sp.color, isNew: false };
    }

    const id = `S${this.nextId++}`;
    const color = this.colors[this.speakers.length % this.colors.length];
    this.speakers.push({ id, name: "", color, centroid: Float64Array.from(emb), count: 1 });
    this.lastSpeakerId = id;
    this.utterances.push({ emb: Float64Array.from(emb), assignedId: id });
    return { id, name: "", color, isNew: true };
  }

  rename(speakerId, newName) {
    const speaker = this.speakers.find((s) => s.id === speakerId);
    if (speaker) speaker.name = newName;
  }

  merge(fromId, toId) {
    const from = this.speakers.find((s) => s.id === fromId);
    const to = this.speakers.find((s) => s.id === toId);
    if (!from || !to || fromId === toId) return false;
    const wf = Math.min(from.count, 50),
      wt = Math.min(to.count, 50);
    const c = to.centroid;
    let nn = 0;
    for (let i = 0; i < c.length; i++) {
      c[i] = (c[i] * wt + from.centroid[i] * wf) / (wt + wf);
      nn += c[i] * c[i];
    }
    nn = Math.sqrt(nn) || 1;
    for (let i = 0; i < c.length; i++) c[i] /= nn;
    to.count += from.count;
    this.speakers = this.speakers.filter((s) => s.id !== fromId);
    if (this.lastSpeakerId === fromId) this.lastSpeakerId = toId;
    return true;
  }

  displayName(speakerId) {
    const speaker = this.speakers.find((s) => s.id === speakerId);
    if (!speaker) return speakerId;
    return speaker.name || speaker.id;
  }

  // Stop-time global recluster (docs/DIARIZATION.md §2, centroid logic, no DOM).
  // Returns the oldId->newId map that was applied (empty if no change).
  globalRecluster(threshold) {
    const th = threshold ?? this.reclusterThreshold ?? 0.65;
    if (this.utterances.length < 2 || this.speakers.length < 2) return new Map();

    const dim = this.utterances[0].emb.length;
    const centroidMap = new Map();
    const countMap = new Map();
    for (const { emb, assignedId } of this.utterances) {
      if (!centroidMap.has(assignedId)) {
        centroidMap.set(assignedId, new Float64Array(dim));
        countMap.set(assignedId, 0);
      }
      const c = centroidMap.get(assignedId);
      for (let i = 0; i < dim; i++) c[i] += emb[i];
      countMap.set(assignedId, countMap.get(assignedId) + 1);
    }
    const trueCentroids = new Map();
    for (const [id, c] of centroidMap) {
      const n = countMap.get(id);
      let norm = 0;
      for (let i = 0; i < dim; i++) {
        c[i] /= n;
        norm += c[i] * c[i];
      }
      norm = Math.sqrt(norm) || 1;
      const v = new Float64Array(dim);
      for (let i = 0; i < dim; i++) v[i] = c[i] / norm;
      trueCentroids.set(id, v);
    }

    let activeIds = this.speakers.map((s) => s.id).filter((id) => trueCentroids.has(id));
    if (activeIds.length < 2) return new Map();

    const renames = new Map(this.speakers.filter((s) => s.name).map((s) => [s.id, s.name]));

    const mergedInto = new Map(activeIds.map((id) => [id, id]));
    const find = (id) => {
      let r = mergedInto.get(id);
      while (r !== mergedInto.get(r)) r = mergedInto.get(r);
      return r;
    };

    let merged = true;
    while (merged) {
      merged = false;
      let bestSim = -1,
        bestI = null,
        bestJ = null;
      for (let a = 0; a < activeIds.length; a++) {
        const ra = find(activeIds[a]);
        const ca = trueCentroids.get(ra);
        if (!ca) continue;
        for (let b = a + 1; b < activeIds.length; b++) {
          const rb = find(activeIds[b]);
          const cb = trueCentroids.get(rb);
          if (!cb) continue;
          if (ra === rb) continue;
          let sim = 0;
          for (let i = 0; i < dim; i++) sim += ca[i] * cb[i];
          if (sim > bestSim) {
            bestSim = sim;
            bestI = ra;
            bestJ = rb;
          }
        }
      }
      if (bestSim >= th && bestI !== null && bestJ !== null) {
        for (const u of this.utterances) {
          if (find(u.assignedId) === bestJ) u.assignedId = bestI;
        }
        mergedInto.set(bestJ, bestI);
        const newC = new Float64Array(dim);
        let cnt = 0;
        for (const { emb, assignedId } of this.utterances) {
          if (find(assignedId) === bestI) {
            for (let i = 0; i < dim; i++) newC[i] += emb[i];
            cnt++;
          }
        }
        let norm = 0;
        for (let i = 0; i < dim; i++) {
          newC[i] /= cnt || 1;
          norm += newC[i] * newC[i];
        }
        norm = Math.sqrt(norm) || 1;
        const v = new Float64Array(dim);
        for (let i = 0; i < dim; i++) v[i] = newC[i] / norm;
        trueCentroids.set(bestI, v);
        trueCentroids.delete(bestJ);
        merged = true;
      }
    }

    const survivors = [...new Set(activeIds.map((id) => find(id)))];
    survivors.sort((a, b) => {
      const na = parseInt(a.replace(/\D/g, ""), 10) || 0;
      const nb = parseInt(b.replace(/\D/g, ""), 10) || 0;
      return na - nb;
    });

    const oldToNew = new Map();
    survivors.forEach((survivorId, idx) => {
      const newId = `S${idx + 1}`;
      for (const origId of activeIds) {
        if (find(origId) === survivorId) oldToNew.set(origId, newId);
      }
    });

    const anyChange = [...oldToNew.entries()].some(([k, v]) => k !== v);
    if (!anyChange) return new Map();

    const newSpeakers = survivors.map((survivorId, idx) => {
      const newId = `S${idx + 1}`;
      const orig = this.speakers.find((s) => s.id === survivorId);
      let name = "";
      for (const [oldId, renamedName] of renames) {
        if (oldToNew.get(oldId) === newId) {
          name = renamedName;
          break;
        }
      }
      return {
        id: newId,
        name,
        color: orig ? orig.color : this.colors[idx % this.colors.length],
        centroid: trueCentroids.get(survivorId) ?? (orig ? orig.centroid : new Float64Array(dim)),
        count: orig ? orig.count : 1,
      };
    });
    this.speakers = newSpeakers;
    return oldToNew;
  }
}

// =====================================================================
// Fixture builders. Each produces { description, dim, steps, expected }.
// A "step" is one event the Rust port replays. `expected` records the
// observable result after each step (and final state).
// =====================================================================

// Pairwise label-error rate against ground truth. For every unordered pair of
// utterances, the clustering and the ground truth each say "same speaker" or
// "different speaker"; an error is a disagreement. This is the Rand-style metric
// and — unlike per-cluster majority vote — it correctly PENALIZES over-splitting
// (breaking a same-speaker pair into two clusters is a same→different error).
// `assigned` and `truth` are parallel arrays over identify-steps.
function pairwiseLabelError(assigned, truth) {
  const n = assigned.length;
  if (n < 2) return 0;
  let errors = 0,
    pairs = 0;
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      pairs++;
      const sameAssigned = assigned[i] === assigned[j];
      const sameTruth = truth[i] === truth[j];
      if (sameAssigned !== sameTruth) errors++;
    }
  }
  return errors / pairs;
}

// `truth` (optional): ground-truth speaker label per identify step (in order).
// `liveThreshold` (optional): overrides the default 0.45 to model the
// DIARIZATION.md "0.5+ over-splits" regime.
function trackerFixture(description, steps, truth, liveThreshold) {
  const t = new SpeakerTracker();
  if (liveThreshold != null) t.threshold = liveThreshold;
  const out = [];
  let preReclusterLabels = null;
  for (const step of steps) {
    if (step.op === "identify") {
      const r = t.identify(step.emb);
      out.push({ op: "identify", result: { id: r.id, isNew: r.isNew } });
    } else if (step.op === "rename") {
      t.rename(step.id, step.name);
      out.push({ op: "rename", id: step.id, name: step.name });
    } else if (step.op === "merge") {
      const ok = t.merge(step.from, step.to);
      out.push({ op: "merge", from: step.from, to: step.to, ok });
    } else if (step.op === "recluster") {
      preReclusterLabels = t.utterances.map((u) => u.assignedId);
      const map = t.globalRecluster(step.threshold);
      out.push({
        op: "recluster",
        threshold: step.threshold ?? t.reclusterThreshold,
        map: Object.fromEntries(map),
      });
    }
  }
  const finalSpeakers = t.speakers.map((s) => ({ id: s.id, name: s.name, count: s.count }));
  const utteranceLabels = t.utterances.map((u) => u.assignedId);

  const fixture = {
    description,
    dim: DIM,
    threshold: t.threshold,
    reclusterThreshold: t.reclusterThreshold,
    steps: steps.map((s) =>
      s.op === "identify" ? { op: "identify", emb: Array.from(s.emb) } : s
    ),
    expected: { trace: out, finalSpeakers, utteranceLabels },
  };

  if (truth) {
    fixture.truth = truth;
    const before = preReclusterLabels ?? utteranceLabels;
    fixture.accuracy = {
      pairwise_error_before: pairwiseLabelError(before, truth),
      pairwise_error_after: pairwiseLabelError(utteranceLabels, truth),
      n_speakers_before: new Set(before).size,
      n_speakers_after: new Set(utteranceLabels).size,
      n_speakers_true: new Set(truth).size,
    };
  }
  return fixture;
}

// ---- Fixture 1: clean two-speaker conversation (no over-split) ----
function buildCleanTwoSpeaker() {
  const A = prototype(101);
  const B = prototype(202);
  const steps = [];
  // Tight jitter (0.05) keeps within-speaker cosine high → no phantom split.
  let seed = 1000;
  const order = ["A", "B", "A", "B", "A", "B", "B", "A"];
  for (const who of order) {
    const proto = who === "A" ? A : B;
    steps.push({ op: "identify", emb: Array.from(utteranceEmb(proto, seed++, 0.05)) });
  }
  return trackerFixture("clean two-speaker conversation, no over-split", steps);
}

// ---- Fixture 2: rename then merge-by-rename ----
function buildRenameMerge() {
  const A = prototype(303);
  const B = prototype(404);
  const steps = [];
  let seed = 2000;
  steps.push({ op: "identify", emb: Array.from(utteranceEmb(A, seed++, 0.05)) }); // S1
  steps.push({ op: "identify", emb: Array.from(utteranceEmb(B, seed++, 0.05)) }); // S2
  steps.push({ op: "identify", emb: Array.from(utteranceEmb(A, seed++, 0.05)) }); // S1
  steps.push({ op: "rename", id: "S1", name: "Alice" });
  steps.push({ op: "rename", id: "S2", name: "Bob" });
  // Now a phantom: a noisy A embedding that splits to S3.
  steps.push({ op: "identify", emb: Array.from(utteranceEmb(A, seed++, 0.9)) }); // likely S3
  // Merge the phantom S3 back into Alice (S1).
  steps.push({ op: "merge", from: "S3", to: "S1" });
  return trackerFixture("rename two speakers, then merge a phantom split", steps);
}

// ---- Fixture 3: merge into a still-empty target / no-op merges ----
function buildMergeEdgeCases() {
  const A = prototype(505);
  const B = prototype(606);
  const steps = [];
  let seed = 3000;
  steps.push({ op: "identify", emb: Array.from(utteranceEmb(A, seed++, 0.05)) }); // S1
  steps.push({ op: "identify", emb: Array.from(utteranceEmb(B, seed++, 0.05)) }); // S2
  steps.push({ op: "merge", from: "S1", to: "S1" }); // self-merge → false
  steps.push({ op: "merge", from: "S9", to: "S1" }); // missing from → false
  steps.push({ op: "merge", from: "S2", to: "S1" }); // real merge → true
  return trackerFixture("merge edge cases: self, missing, real", steps);
}

// ---- Fixture 4: messy 2-speaker meeting, over-split + recluster repair ----
// A 20-utterance A/B conversation at MESSY_JITTER. The online tracker over-splits
// (same-speaker utts dip below 0.45 against the running centroid → phantoms);
// the stop-time recluster compares robust true centroids and collapses each
// speaker's phantoms back. Ground truth = which prototype each utterance came
// from. We expect ler_after < ler_before and n_speakers_after closer to 2.
function buildMessyTwoSpeaker() {
  const A = prototype(707);
  const B = prototype(808);
  const steps = [];
  const truth = [];
  let seed = 4000;
  // Interleaved A/B, 10 each. Seeded so the stream is fully deterministic.
  const order = "ABABABABABABABABABAB".split("");
  for (const who of order) {
    const proto = who === "A" ? A : B;
    steps.push({ op: "identify", emb: Array.from(utteranceEmb(proto, seed++, MESSY_JITTER)) });
    truth.push(who);
  }
  steps.push({ op: "recluster", threshold: 0.65 });
  return trackerFixture(
    "messy 2-speaker meeting, recluster repairs over-split",
    steps,
    truth,
    OVERSPLIT_THRESHOLD
  );
}

// ---- Fixture 5: messy 3-speaker meeting, over-split + recluster repair ----
// Three speakers, 8 utts each, MESSY_JITTER. Harder: recluster must not collapse
// distinct speakers while still merging each speaker's phantoms.
function buildMessyThreeSpeaker() {
  const A = prototype(707);
  const B = prototype(808);
  const C = prototype(909);
  const steps = [];
  const truth = [];
  let seed = 6000;
  const order = "ABCABCABCABCABCABCABCABC".split(""); // 24 utts, 8 each
  for (const who of order) {
    const proto = who === "A" ? A : who === "B" ? B : C;
    steps.push({ op: "identify", emb: Array.from(utteranceEmb(proto, seed++, MESSY_JITTER)) });
    truth.push(who);
  }
  steps.push({ op: "recluster", threshold: 0.65 });
  return trackerFixture(
    "messy 3-speaker meeting, recluster repairs over-split",
    steps,
    truth,
    OVERSPLIT_THRESHOLD
  );
}

// ---- Fixture 6: recluster preserves a rename across a merge ----
// Speaker A over-splits at the too-high live threshold into S1 + phantoms; the
// user renames the FIRST A cluster (S1) to "Alice"; the stop-time recluster
// merges A's phantoms into the canonical id, and the "Alice" name MUST survive
// onto whatever id the merged group maps to. This is a PRD exit criterion
// (renames/merges survive recluster), so it must exercise a REAL merge.
function buildReclusterPreservesRename() {
  const A = prototype(707);
  const B = prototype(808);
  const steps = [];
  const truth = [];
  let seed = 4000; // same seed stream as messy_two_speaker → known over-split
  // Same interleaved A/B/messy stream that over-splits to many clusters, but we
  // rename S1 (an A cluster) to "Alice" partway through and recluster at the end.
  const order = "ABABABABABABABABABAB".split("");
  order.forEach((who, idx) => {
    const proto = who === "A" ? A : B;
    steps.push({ op: "identify", emb: Array.from(utteranceEmb(proto, seed++, MESSY_JITTER)) });
    truth.push(who);
    // After the first A and first B are seen, rename S1 → "Alice".
    if (idx === 2) steps.push({ op: "rename", id: "S1", name: "Alice" });
  });
  steps.push({ op: "recluster", threshold: 0.65 });
  return trackerFixture(
    "recluster preserves the Alice rename on the merged group",
    steps,
    truth,
    OVERSPLIT_THRESHOLD
  );
}

const fixtures = {
  tracker: {
    clean_two_speaker: buildCleanTwoSpeaker(),
    rename_then_merge: buildRenameMerge(),
    merge_edge_cases: buildMergeEdgeCases(),
  },
  recluster: {
    messy_two_speaker: buildMessyTwoSpeaker(),
    messy_three_speaker: buildMessyThreeSpeaker(),
    recluster_preserves_rename: buildReclusterPreservesRename(),
  },
};

mkdirSync(join(HERE, "..", "tracker"), { recursive: true });
mkdirSync(join(HERE, "..", "recluster"), { recursive: true });

for (const [name, fx] of Object.entries(fixtures.tracker)) {
  const p = join(HERE, "..", "tracker", `${name}.json`);
  writeFileSync(p, JSON.stringify(fx, null, 1));
  console.log("wrote", p, "—", fx.expected.finalSpeakers.length, "final speakers");
}
for (const [name, fx] of Object.entries(fixtures.recluster)) {
  const p = join(HERE, "..", "recluster", `${name}.json`);
  writeFileSync(p, JSON.stringify(fx, null, 1));
  const a = fx.accuracy;
  console.log(
    "wrote", p,
    "\n   speakers:", a.n_speakers_before, "→", a.n_speakers_after, "(true", a.n_speakers_true + ")",
    "| pairwise-err:", a.pairwise_error_before.toFixed(3), "→", a.pairwise_error_after.toFixed(3),
    "\n   final:", fx.expected.finalSpeakers.map((s) => s.id + (s.name ? `=${s.name}` : "")).join(",")
  );
}
