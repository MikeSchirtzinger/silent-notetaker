/**
 * capture.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * Role: browser audio capture only — `getUserMedia` (mic @16 kHz mono with echo
 * cancel / noise suppress / AGC), `getDisplayMedia` (tab/system audio +
 * dual-channel worklet mix, stream-ended handling), and the AudioWorklet graph
 * (Appendix A rows 4, 5, 26). It emits typed `AudioChunk` events to the Rust
 * core; it owns NO policy (chunk sizes, engine selection, and VAD thresholds all
 * arrive from Rust). Screenshot capture (row 26) also lands here.
 *
 * # What lives here vs what stays in index.html / *-engine.js
 *
 * capture.js owns the browser capture APIs:
 *   - getUserMedia @16 kHz mono (echoCancellation/noiseSuppression/autoGainControl)
 *   - the AudioContext + AudioWorklet "capture-processor" setup (GainNode mixer so
 *     mic and tab audio both feed the worklet)
 *   - getDisplayMedia + dual-channel GainNode mix + stream-ended handling
 *   - startScreenshotCapture / captureFrame (15 s interval, JPEG, perceptual hash
 *     dedup) / stopScreenshotCapture
 *
 * index.html's TranscriptionManager delegates audio capture and screenshot to a
 * CaptureGraph instance; the engine-specific feed callbacks are supplied by the
 * engine loaders that own the decoding policy.
 *
 * # Phase 1 relocation
 *
 * This code was previously inlined as methods on `TranscriptionManager` in
 * index.html. The relocation is pure (logic unchanged); TranscriptionManager
 * now instantiates CaptureGraph and delegates.
 */

/**
 * Manages the browser audio capture graph (mic + optional tab/system audio) and
 * the periodic tab-video screenshot pipeline.
 *
 * Constructor options:
 *   onSamples          (samples: Float32Array) => void
 *     Called for every AudioWorklet render quantum (128 samples @16 kHz = 8 ms).
 *     The engine loaders subscribe to this to drive their policy feed loops.
 *   onScreenshot       (base64DataUrl: string, timestamp: number) => void
 *     Called after each unique frame is captured and stored. Optional.
 *   getStorage         () => storageObject
 *     Returns the live storage object for persisting screenshots. Optional;
 *     if absent, screenshots are still emitted via onScreenshot but not persisted.
 *   getMeetingId       () => number|null
 *     Returns the current meeting id for the screenshot storage record. Optional.
 *   onSystemAudioEnded () => void
 *     Called when the browser's tab-sharing indicator is dismissed by the user
 *     (the audio track's `onended` event). Optional.
 */
export class CaptureGraph {
  constructor({ onSamples, onScreenshot, getStorage, getMeetingId, onSystemAudioEnded } = {}) {
    this._onSamples           = onSamples           || null;
    this._onScreenshot        = onScreenshot        || null;
    this._getStorage          = getStorage          || null;
    this._getMeetingId        = getMeetingId        || null;
    this._onSystemAudioEnded  = onSystemAudioEnded  || null;

    // AudioContext graph (set by startMic / startAudioCapture).
    this.audioContext   = null;
    this.audioInputNode = null;   // GainNode mixer; tab audio connects here too
    this.processor      = null;   // AudioWorkletNode "capture-processor"
    this.mediaStream    = null;   // mic MediaStream
    this.isRecording    = false;

    // System audio (set by addSystemAudio).
    this.systemStream   = null;
    this.systemSource   = null;
    this.systemGain     = null;

    // Screenshot pipeline (set by startScreenshotCapture).
    this.videoTrack       = null;
    this.videoElement     = null;
    this.captureCanvas    = null;
    this.captureCtx       = null;
    this.screenshotInterval = null;
    this.lastFrameHash    = null;
  }

  /**
   * Request microphone access at 16 kHz mono with echo-cancel/noise-suppress/AGC
   * and wire the AudioWorklet capture graph. Returns the opened MediaStream.
   *
   * Callers that need to pre-open the mic before building the graph (e.g. engine
   * loaders that need the stream to verify permissions) may call this; the stream
   * is stored in `this.mediaStream` and reused when `startAudioCapture` is called.
   *
   * @returns {Promise<MediaStream>}
   */
  async startMic() {
    this.mediaStream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: 1,
        sampleRate: 16000,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      }
    });
    return this.mediaStream;
  }

  /**
   * Shared audio capture pipeline used by every engine. Uses AudioWorklet (runs
   * in the audio render thread) so samples don't drop when the main thread stalls.
   * Mic and tab audio both feed `this.audioInputNode`, a GainNode mixer; the
   * worklet posts a Float32Array per render quantum.
   *
   * If `this.mediaStream` is already set (e.g. from a prior `startMic()` call),
   * the existing stream is reused. Otherwise a new mic stream is opened.
   *
   * The `onSamples` callback supplied at construction time receives every quantum.
   * It is guarded by `this.isRecording`, so samples delivered between a stop() and
   * the next start() are dropped.
   *
   * @param {(samples: Float32Array) => void} [onSamplesOverride]
   *   If provided, overrides the constructor `onSamples` for this session only.
   *   Useful when the engine loader needs to swap the feed target (e.g. resume
   *   with a different engine).
   */
  async startAudioCapture(onSamplesOverride) {
    const onSamples = onSamplesOverride || this._onSamples;
    if (!this.mediaStream) await this.startMic();

    this.audioContext = new AudioContext({ sampleRate: 16000 });
    if (this.audioContext.state === 'suspended') {
      await this.audioContext.resume();
    }

    // Mixer node — addSystemAudio() also connects into this so mic + tab audio sum here.
    this.audioInputNode = this.audioContext.createGain();
    this.audioInputNode.gain.value = 1.0;

    const micSource = this.audioContext.createMediaStreamSource(this.mediaStream);
    micSource.connect(this.audioInputNode);

    const workletSrc = `
      class CaptureProcessor extends AudioWorkletProcessor {
        process(inputs) {
          const input = inputs[0];
          if (input.length > 0 && input[0].length > 0) {
            this.port.postMessage(input[0]);
          }
          return true;
        }
      }
      registerProcessor("capture-processor", CaptureProcessor);
    `;
    const workletUrl = URL.createObjectURL(new Blob([workletSrc], { type: 'application/javascript' }));
    await this.audioContext.audioWorklet.addModule(workletUrl);
    URL.revokeObjectURL(workletUrl);

    this.processor = new AudioWorkletNode(this.audioContext, 'capture-processor');
    this.processor.port.onmessage = (e) => {
      if (!this.isRecording) return;
      if (onSamples) onSamples(new Float32Array(e.data));
    };

    // Silent gain keeps the graph live without echoing audio back to speakers.
    const silentGain = this.audioContext.createGain();
    silentGain.gain.value = 0;

    this.audioInputNode.connect(this.processor);
    this.processor.connect(silentGain);
    silentGain.connect(this.audioContext.destination);

    this.isRecording = true;
  }

  /**
   * Add tab / system audio to the capture graph.
   *
   * Opens a `getDisplayMedia` prompt (video: true required by Chrome to expose
   * tab audio). The first video track is handed off to `startScreenshotCapture`;
   * extra video tracks are stopped immediately. The audio track is fed through a
   * +1.5 dB gain node into the same GainNode mixer as the mic, so the worklet
   * receives the dual-channel mix in every quantum. When the user dismisses the
   * browser's tab-sharing indicator, `onended` fires on the audio track; this
   * calls `removeSystemAudio()` and then the `onSystemAudioEnded` callback.
   *
   * Throws if `startAudioCapture` has not been called first (the mic graph must
   * exist before tab audio can be mixed into it).
   */
  async addSystemAudio() {
    if (!this.audioContext || !this.processor) {
      throw new Error('Start recording first, then add tab audio.');
    }

    // Request tab/screen sharing — video:true required by Chrome to enable tab audio
    const displayStream = await navigator.mediaDevices.getDisplayMedia({
      audio: {
        channelCount: 1,
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
      },
      video: true,
    });

    // Keep first video track for screenshot capture, stop the rest
    const videoTracks = displayStream.getVideoTracks();
    if (videoTracks.length > 0) {
      this.startScreenshotCapture(videoTracks[0]);
      for (let i = 1; i < videoTracks.length; i++) videoTracks[i].stop();
    }

    const audioTracks = displayStream.getAudioTracks();
    if (audioTracks.length === 0) {
      throw new Error('No audio track from shared tab. Make sure "Share tab audio" is checked.');
    }

    // Store for cleanup
    this.systemStream = displayStream;

    // Create source and connect to the same processor — Web Audio sums (mixes) inputs
    const systemSource = this.audioContext.createMediaStreamSource(
      new MediaStream(audioTracks)
    );

    // Optional: add gain to balance system vs mic levels
    this.systemGain = this.audioContext.createGain();
    this.systemGain.gain.value = 1.5; // boost tab audio slightly
    systemSource.connect(this.systemGain);
    this.systemGain.connect(this.audioInputNode);

    this.systemSource = systemSource;

    // Detect when user stops sharing from browser chrome
    audioTracks[0].onended = () => {
      this.removeSystemAudio();
      if (this._onSystemAudioEnded) this._onSystemAudioEnded();
    };
  }

  /** Disconnect and tear down the system audio branch (inverse of addSystemAudio). */
  removeSystemAudio() {
    this.stopScreenshotCapture();
    if (this.systemSource) { try { this.systemSource.disconnect(); } catch (_) {} this.systemSource = null; }
    if (this.systemGain)   { try { this.systemGain.disconnect();   } catch (_) {} this.systemGain   = null; }
    if (this.systemStream) { this.systemStream.getTracks().forEach(t => t.stop()); this.systemStream = null; }
  }

  /**
   * Stop the audio capture graph. The mic stream and AudioContext are closed and
   * nulled. The system audio branch is torn down first via `removeSystemAudio()`.
   * Models stay loaded; callers can re-open the graph for a new session.
   */
  stop() {
    this.isRecording = false;
    this.removeSystemAudio();
    if (this.processor)   { try { this.processor.disconnect();   } catch (_) {} this.processor   = null; }
    if (this.audioContext){ try { this.audioContext.close();       } catch (_) {} this.audioContext = null; }
    if (this.mediaStream) { this.mediaStream.getTracks().forEach(t => t.stop()); this.mediaStream = null; }
  }

  // ── Screenshot pipeline ──────────────────────────────────────────────────

  /**
   * Start capturing screenshots from a tab video track. Sets up a hidden
   * `<video>` element and a canvas, then fires `captureFrame` every 15 seconds
   * (and once after 2 s to capture the initial state). Stopped automatically by
   * `removeSystemAudio()` and `stop()`.
   *
   * @param {MediaStreamTrack} videoTrack  First video track from getDisplayMedia.
   */
  startScreenshotCapture(videoTrack) {
    this.videoTrack = videoTrack;

    // Create hidden video element to render the track
    this.videoElement = document.createElement('video');
    this.videoElement.srcObject = new MediaStream([videoTrack]);
    this.videoElement.muted = true;
    this.videoElement.play();

    // Create canvas for frame capture
    this.captureCanvas = document.createElement('canvas');
    this.captureCtx = this.captureCanvas.getContext('2d', { willReadFrequently: true });

    // Capture every 15 seconds
    this.screenshotInterval = setInterval(() => this.captureFrame(), 15000);
    // Also capture first frame after short delay
    setTimeout(() => this.captureFrame(), 2000);
  }

  /**
   * Capture one frame from the tab video, run a perceptual-hash dedup check,
   * convert to JPEG, store via the `getStorage()` callback, and emit via
   * `onScreenshot`. Called automatically at 15 s intervals while tab sharing is
   * active. Safe to call manually for an on-demand capture.
   */
  captureFrame() {
    if (!this.videoElement || this.videoElement.readyState < 2) return;

    // Scale to max 640px wide for thumbnails
    const vw = this.videoElement.videoWidth;
    const vh = this.videoElement.videoHeight;
    if (vw === 0 || vh === 0) return;

    const scale = Math.min(640 / vw, 1);
    const w = Math.round(vw * scale);
    const h = Math.round(vh * scale);

    this.captureCanvas.width = w;
    this.captureCanvas.height = h;
    this.captureCtx.drawImage(this.videoElement, 0, 0, w, h);

    // Quick similarity check: sample pixels and compute a rough hash
    const imageData = this.captureCtx.getImageData(0, 0, w, h).data;
    let hash = 0;
    const step = Math.max(1, Math.floor(imageData.length / 256));
    for (let i = 0; i < imageData.length; i += step) {
      hash = ((hash << 5) - hash + imageData[i]) | 0;
    }

    // Skip if frame is very similar to last one
    if (this.lastFrameHash !== null && this.lastFrameHash === hash) return;
    this.lastFrameHash = hash;

    // Convert to JPEG blob
    this.captureCanvas.toBlob((blob) => {
      if (!blob) return;

      // Convert to base64 for storage and sending
      const reader = new FileReader();
      reader.onloadend = () => {
        const base64 = reader.result; // data:image/jpeg;base64,...
        const timestamp = Date.now();

        // Store in IndexedDB (Rust storage — base64 data-URL string, stored as
        // UTF-8 bytes with the `base64` encoding marker).
        const storage   = this._getStorage   ? this._getStorage()   : null;
        const meetingId = this._getMeetingId ? this._getMeetingId() : null;
        if (storage && meetingId != null) {
          storage.addScreenshot(meetingId, timestamp, base64, w, h)
            .catch(err => console.warn('[screenshot] storage add failed', err));
        }

        // Notify main app (it will send to Claude if connected)
        if (this._onScreenshot) this._onScreenshot(base64, timestamp);
      };
      reader.readAsDataURL(blob);
    }, 'image/jpeg', 0.7);
  }

  /** Stop the screenshot interval and release the video element and canvas. */
  stopScreenshotCapture() {
    if (this.screenshotInterval) { clearInterval(this.screenshotInterval); this.screenshotInterval = null; }
    if (this.videoTrack)   { this.videoTrack.stop(); this.videoTrack = null; }
    if (this.videoElement) { this.videoElement.srcObject = null; this.videoElement = null; }
    this.captureCanvas    = null;
    this.captureCtx       = null;
    this.lastFrameHash    = null;
  }
}
