// 采集 worklet：把麦克风（或测试音）重采样成 48 kHz 单声道，
// 每 480 采样（10 ms）通过 port 发给主线程。
class CaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = (options && options.processorOptions) || {};
    this.frameSamples = opts.frameSamples || 480;
    this.mode = opts.mode || "mic";
    this.buf = new Float32Array(this.frameSamples);
    this.n = 0;
    this.phase = 0;
    this.toneHz = 440;
    this.port.onmessage = (event) => {
      const data = event.data || {};
      if (data.mode) this.mode = data.mode;
      if (data.toneHz) this.toneHz = data.toneHz;
    };
  }

  process(inputs) {
    const channel = (inputs[0] && inputs[0][0]) || null;
    const quantum = channel ? channel.length : 128;

    for (let i = 0; i < quantum; i++) {
      let v;
      if (this.mode === "tone") {
        // 测试音：440 Hz，约 -16 dBFS，用来在没有麦克风的机器上验证链路
        this.phase += (2 * Math.PI * this.toneHz) / sampleRate;
        if (this.phase > 2 * Math.PI) this.phase -= 2 * Math.PI;
        v = 0.16 * Math.sin(this.phase);
      } else {
        v = channel ? channel[i] : 0;
      }

      this.buf[this.n++] = v;
      if (this.n === this.frameSamples) {
        this.port.postMessage(this.buf.slice(0));
        this.n = 0;
      }
    }
    return true;
  }
}

registerProcessor("capture-processor", CaptureProcessor);
