/* A/B harness: feed a WAV through getUserMedia override; scrape transcript+speakers. */
window.__ab = {
  async setup(url) {
    const ctx = new AudioContext({ sampleRate: 16000 });
    try { await ctx.resume(); } catch (_) {}
    const buf = await (await fetch(url)).arrayBuffer();
    const audio = await ctx.decodeAudioData(buf);
    const dest = ctx.createMediaStreamDestination();
    const src = ctx.createBufferSource();
    src.buffer = audio;
    src.connect(dest);
    // RMS meter so we can PROVE audio is flowing (fake-mic silence trap).
    const an = ctx.createAnalyser();
    an.fftSize = 2048;
    src.connect(an);
    this.an = an;
    this.ctx = ctx; this.src = src; this.stream = dest.stream;
    this.started = false; this.duration = audio.duration; this.ended = false;
    src.onended = () => { this.ended = true; };
    if (!this._patched) {
      // Hand the app a LIVE-but-silent stream; audio rolls only on __ab.begin(),
      // so model-load time can never eat the head of the clip for either engine.
      navigator.mediaDevices.getUserMedia = async () => this.stream;
      this._patched = true;
    }
    return JSON.stringify({ duration: audio.duration, ctxState: ctx.state });
  },
  begin() {
    if (!this.started) { this.src.start(); this.started = true; }
    return 'rolling';
  },
  rms() {
    if (!this.an) return -1;
    const d = new Float32Array(this.an.fftSize);
    this.an.getFloatTimeDomainData(d);
    let s = 0; for (let i = 0; i < d.length; i++) s += d[i] * d[i];
    return Math.sqrt(s / d.length);
  },
  status() {
    return JSON.stringify({
      started: this.started, ended: this.ended,
      ctxState: this.ctx ? this.ctx.state : null,
      rms: Math.round(this.rms() * 10000) / 10000,
      items: document.querySelectorAll('#transcriptContent .transcript-item').length,
    });
  },
  scrape() {
    return JSON.stringify(
      Array.from(document.querySelectorAll('#transcriptContent .transcript-item')).map((el) => ({
        spk: (el.querySelector('.speaker-tag') || {}).textContent || null,
        live: el.classList.contains('transcript-live'),
        text: ((el.querySelector('.transcript-text') || {}).textContent || '').trim(),
      }))
    );
  },
};
'ab-loaded';
