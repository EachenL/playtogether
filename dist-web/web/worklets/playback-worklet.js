// 播放 worklet：环形缓冲 + 固定目标填充（60 ms），避免抖动变成断音。
class PlaybackProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = (options && options.processorOptions) || {};
    this.size = Math.round(sampleRate * 2);          // 2 秒环
    this.buf = new Float32Array(this.size);
    this.w = 0;
    this.r = 0;
    this.targetFill = Math.round(sampleRate * (opts.targetMs || 60) / 1000);
    this.maxFill = Math.round(sampleRate * 0.8);
    this.playing = false;

    this.port.onmessage = (event) => {
      const chunk = event.data;
      if (!chunk || !chunk.length) return;

      for (let i = 0; i < chunk.length; i++) {
        this.buf[this.w] = chunk[i];
        this.w = (this.w + 1) % this.size;
      }

      // 溢出保护：写超了就丢最老的
      const fill = (this.w - this.r + this.size) % this.size;
      if (fill > this.maxFill) {
        this.r = (this.w - this.targetFill + this.size) % this.size;
      }
    };
  }

  process(inputs, outputs) {
    const out = outputs[0];
    const left = out[0];
    const right = out[1] || null;
    const frames = left.length;

    const fill = (this.w - this.r + this.size) % this.size;
    // 攒够目标填充才开始播，避免一上来就断续
    if (!this.playing && fill >= this.targetFill) this.playing = true;

    for (let i = 0; i < frames; i++) {
      let v = 0;
      if (this.playing) {
        v = this.buf[this.r];
        this.r = (this.r + 1) % this.size;
        if (this.r === this.w) this.playing = false;   // 播空了，等下一次攒够
      }
      left[i] = v;
      if (right) right[i] = v;
    }
    return true;
  }
}

registerProcessor("playback-processor", PlaybackProcessor);
