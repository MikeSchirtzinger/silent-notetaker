// Prove the JS front-end matches the Python ground truth, end to end:
//   feature-level (max abs diff vs ref.feat) AND embedding-level (cosine vs ref.emb).
import * as ort from "onnxruntime-node";
import { computeLogMel } from "./melfeat.mjs";
import fs from "node:fs";

const mel = JSON.parse(fs.readFileSync("mel_fb.json", "utf8")).matrix;
const sess = await ort.InferenceSession.create("titanet.onnx");

function cosine(a, b) {
  let d = 0, na = 0, nb = 0;
  for (let i = 0; i < a.length; i++) { d += a[i] * b[i]; na += a[i] * a[i]; nb += b[i] * b[i]; }
  return d / (Math.sqrt(na) * Math.sqrt(nb));
}

let worstFeat = 0, worstEmbCos = 1;
for (const f of fs.readdirSync("ref")) {
  const ref = JSON.parse(fs.readFileSync(`ref/${f}`, "utf8"));
  const samples = Float64Array.from(ref.samples);
  const { data, nMels, T } = computeLogMel(samples, mel);

  // feature parity
  let maxAbs = 0;
  for (let m = 0; m < nMels; m++)
    for (let t = 0; t < T; t++)
      maxAbs = Math.max(maxAbs, Math.abs(data[m * T + t] - ref.feat[m][t]));

  // embedding parity
  const audio = new ort.Tensor("float32", data, [1, nMels, T]);
  const length = new ort.Tensor("int64", BigInt64Array.from([BigInt(T)]), [1]);
  const out = await sess.run({ audio_signal: audio, length });
  const emb = out.embs.data;
  const cos = cosine(emb, ref.emb);

  worstFeat = Math.max(worstFeat, maxAbs);
  worstEmbCos = Math.min(worstEmbCos, cos);
  console.log(`${f.padEnd(34)} T=${T}  feat maxAbsDiff=${maxAbs.toExponential(2)}  embCos=${cos.toFixed(6)}`);
}
console.log("─".repeat(70));
console.log(`WORST feature maxAbsDiff=${worstFeat.toExponential(2)}   WORST embedding cosine=${worstEmbCos.toFixed(6)}`);
console.log(worstEmbCos > 0.999 && worstFeat < 1e-3 ? "✅ JS front-end MATCHES Python" : "❌ mismatch — needs fixing");
