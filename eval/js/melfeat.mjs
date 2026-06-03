// NeMo TitaNet-small mel front-end — pure JS, runs identically in Node and browser.
// Mirrors the Python recipe validated in speaker-bench:
//   preemph 0.97 -> STFT(n_fft=512, win=400, hop=160, hann periodic, center/reflect)
//   -> power -> slaney 80-mel (baked matrix) -> log(x+2^-24) -> per-feature norm.
// Validated to reproduce sherpa-onnx (gap 0.785, EER 0%, 0.94 cos to sherpa).

const NFFT = 512, WIN = 400, HOP = 160, PAD = 256, NFREQ = 257, LOG_GUARD = 2 ** -24;

export function preemphasis(x, c = 0.97) {
  const y = new Float64Array(x.length);
  y[0] = x[0];
  for (let i = 1; i < x.length; i++) y[i] = x[i] - c * x[i - 1];
  return y;
}

// numpy reflect padding (edge sample not repeated)
function reflectPad(x, pad) {
  const n = x.length, out = new Float64Array(n + 2 * pad);
  for (let i = 0; i < n; i++) out[pad + i] = x[i];
  for (let i = 0; i < pad; i++) { out[pad - 1 - i] = x[i + 1]; out[pad + n + i] = x[n - 2 - i]; }
  return out;
}

// hann(400, periodic) centered into a length-512 buffer (= librosa pad_center)
function hannPadded() {
  const w = new Float64Array(NFFT);
  const left = (NFFT - WIN) >> 1;
  for (let n = 0; n < WIN; n++) w[left + n] = 0.5 - 0.5 * Math.cos((2 * Math.PI * n) / WIN);
  return w;
}

// in-place iterative radix-2 FFT
function fft(re, im) {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { const tr = re[i]; re[i] = re[j]; re[j] = tr; const ti = im[i]; im[i] = im[j]; im[j] = ti; }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const ang = (-2 * Math.PI) / len, wr = Math.cos(ang), wi = Math.sin(ang), half = len >> 1;
    for (let i = 0; i < n; i += len) {
      let cwr = 1, cwi = 0;
      for (let k = 0; k < half; k++) {
        const a = i + k, b = i + k + half;
        const vr = re[b] * cwr - im[b] * cwi, vi = re[b] * cwi + im[b] * cwr;
        re[b] = re[a] - vr; im[b] = im[a] - vi; re[a] += vr; im[a] += vi;
        const ncwr = cwr * wr - cwi * wi; cwi = cwr * wi + cwi * wr; cwr = ncwr;
      }
    }
  }
}

const WINDOW = hannPadded();

/**
 * @param {Float32Array|Float64Array} samples  16kHz mono PCM
 * @param {number[][]} mel  80x257 slaney mel matrix (from mel_fb.json)
 * @returns {{data: Float32Array, nMels: number, T: number}}  log-mel [80,T] row-major
 */
export function computeLogMel(samples, mel) {
  const nMels = mel.length;
  const x = reflectPad(preemphasis(samples), PAD);
  const T = 1 + Math.floor((x.length - NFFT) / HOP);
  const feat = new Float64Array(nMels * T);
  const re = new Float64Array(NFFT), im = new Float64Array(NFFT);
  const power = new Float64Array(NFREQ);
  for (let t = 0; t < T; t++) {
    const off = t * HOP;
    for (let i = 0; i < NFFT; i++) { re[i] = x[off + i] * WINDOW[i]; im[i] = 0; }
    fft(re, im);
    for (let k = 0; k < NFREQ; k++) power[k] = re[k] * re[k] + im[k] * im[k];
    for (let m = 0; m < nMels; m++) {
      const row = mel[m];
      let s = 0;
      for (let k = 0; k < NFREQ; k++) s += row[k] * power[k];
      feat[m * T + t] = Math.log(s + LOG_GUARD);
    }
  }
  // per-feature (per mel bin) mean/std normalization across time
  for (let m = 0; m < nMels; m++) {
    const base = m * T;
    let mean = 0;
    for (let t = 0; t < T; t++) mean += feat[base + t];
    mean /= T;
    let v = 0;
    for (let t = 0; t < T; t++) { const d = feat[base + t] - mean; v += d * d; }
    const std = Math.sqrt(v / T) + 1e-5;
    for (let t = 0; t < T; t++) feat[base + t] = (feat[base + t] - mean) / std;
  }
  const out = new Float32Array(nMels * T);
  for (let i = 0; i < out.length; i++) out[i] = feat[i];
  return { data: out, nMels, T };
}
