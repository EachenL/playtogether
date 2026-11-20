// playTogether 网页版客户端
// 采集：AudioWorklet → 480 采样/帧 → WebSocket 二进制 → 服务器
// 播放：WebSocket → Int16→Float32 → AudioWorklet 环形缓冲 → 扬声器
// 注：浏览器不能发 UDP，所以走 WebSocket 到 jt-webd，再由它转发成 JT01/UDP。

const FRAME_SAMPLES = 480;
const FRAME_MS = 10;

const $ = (id) => document.getElementById(id);
const ui = {
  mode: $("mode"),
  join: $("join"),
  leave: $("leave"),
  state: $("state"),
  rtt: $("m-rtt"),
  tx: $("m-tx"),
  rx: $("m-rx"),
  lat: $("m-lat"),
  selfBar: $("lv-self"),
  peerBar: $("lv-peer"),
  selfTxt: $("lv-self-txt"),
  peerTxt: $("lv-peer-txt"),
  log: $("log"),
  hint: $("hint"),
};

const runtime = {
  ws: null,
  ctx: null,
  stream: null,
  captureNode: null,
  playbackNode: null,
  muteGain: null,
  joined: false,
  tx: 0,
  rx: 0,
  rttMs: null,
  selfLevel: 0,
  peerLevel: 0,
  startedAt: 0,
};

window.__jam = runtime;

/* ── 小工具 ─────────────────────────────── */
function log(msg, kind) {
  const line = document.createElement("div");
  line.className = "log-line" + (kind ? " " + kind : "");
  const t = new Date().toLocaleTimeString("zh-CN", { hour12: false });
  line.textContent = `[${t}] ${msg}`;
  ui.log.prepend(line);
  while (ui.log.childElementCount > 40) ui.log.lastElementChild.remove();
}

function setState(kind, text) {
  ui.state.className = "chip" + (kind ? " " + kind : "");
  ui.state.textContent = text;
}

function fmtDb(v) {
  if (!v || v <= 0) return "—";
  const db = 20 * Math.log10(v);
  return db.toFixed(0) + " dB";
}

function paintMeter(el, level) {
  const bars = el.children;
  const active = Math.round(Math.min(1, Math.max(0, level)) * bars.length * 3);
  for (let i = 0; i < bars.length; i++) {
    const on = i < active;
    bars[i].style.background = !on
      ? "rgba(255,255,255,.08)"
      : i > bars.length - 4
        ? "linear-gradient(180deg,#ff6b6b,#ffb454)"
        : "linear-gradient(180deg,#3ddc97,#22d3ee)";
  }
}

function rms(chunk) {
  let sum = 0;
  for (let i = 0; i < chunk.length; i++) sum += chunk[i] * chunk[i];
  return Math.sqrt(sum / chunk.length);
}

/* ── 音频 ───────────────────────────────── */
async function setupAudio(mode) {
  const ctx = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
  runtime.ctx = ctx;
  await ctx.resume();

  await ctx.audioWorklet.addModule("worklets/capture-worklet.js");
  await ctx.audioWorklet.addModule("worklets/playback-worklet.js");

  // 播放链：worklet → 扬声器
  const playback = new AudioWorkletNode(ctx, "playback-processor", {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [2],
    processorOptions: { targetMs: 60 },
  });
  playback.connect(ctx.destination);
  runtime.playbackNode = playback;

  // 采集链：mic（或纯测试音）→ worklet → 静音 gain → destination（保证被拉取）
  const capture = new AudioWorkletNode(ctx, "capture-processor", {
    numberOfInputs: 1,
    numberOfOutputs: 1,
    outputChannelCount: [1],
    channelCount: 1,
    channelCountMode: "explicit",
    processorOptions: { frameSamples: FRAME_SAMPLES, mode },
  });
  capture.port.postMessage({ mode });

  if (mode === "mic") {
    runtime.stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
        channelCount: 1,
      },
      video: false,
    });
    const src = ctx.createMediaStreamSource(runtime.stream);
    src.connect(capture);
  }

  const mute = ctx.createGain();
  mute.gain.value = 0;
  capture.connect(mute);
  mute.connect(ctx.destination);
  runtime.captureNode = capture;
  runtime.muteGain = mute;

  // 采集帧 → Int16 → WebSocket
  capture.port.onmessage = (event) => {
    const f32 = event.data;
    runtime.selfLevel = rms(f32);
    const i16 = new Int16Array(f32.length);
    for (let i = 0; i < f32.length; i++) {
      const s = Math.max(-1, Math.min(1, f32[i]));
      i16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    if (runtime.ws && runtime.ws.readyState === WebSocket.OPEN) {
      runtime.ws.send(i16.buffer);
      runtime.tx++;
    }
  };

  log(`音频就绪：${ctx.sampleRate} Hz，帧 ${FRAME_SAMPLES} 采样（${FRAME_MS} ms），输出缓冲 60 ms`, "ok");
}

async function teardownAudio() {
  try {
    if (runtime.captureNode) runtime.captureNode.disconnect();
    if (runtime.muteGain) runtime.muteGain.disconnect();
    if (runtime.playbackNode) runtime.playbackNode.disconnect();
    if (runtime.stream) runtime.stream.getTracks().forEach((t) => t.stop());
    if (runtime.ctx && runtime.ctx.state !== "closed") await runtime.ctx.close();
  } catch (err) {
    /* 忽略清理错误 */
  }
  runtime.ctx = null;
  runtime.stream = null;
  runtime.captureNode = null;
  runtime.playbackNode = null;
  runtime.muteGain = null;
}

/* ── 连接 ───────────────────────────────── */
function wsUrl() {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${location.host}/ws`;
}

async function join() {
  if (runtime.joined) return;
  const mode = ui.mode.value;
  ui.join.disabled = true;
  setState("busy", "连接中…");
  log(`正在连接 ${wsUrl()}（${mode === "mic" ? "麦克风" : "测试音"}模式）`);

  try {
    await setupAudio(mode);
  } catch (err) {
    setState("", "未加入");
    log("音频初始化失败：" + (err && err.message ? err.message : err), "err");
    ui.join.disabled = false;
    await teardownAudio();
    return;
  }

  const ws = new WebSocket(wsUrl());
  ws.binaryType = "arraybuffer";
  runtime.ws = ws;

  ws.onopen = () => {
    runtime.joined = true;
    runtime.startedAt = performance.now();
    runtime.tx = 0;
    runtime.rx = 0;
    setState("on", "已加入");
    ui.leave.disabled = false;
    ui.hint.textContent = "已进入房间。让乐队其他人也打开这个页面（或桌面客户端）连同一台服务器。";
    log("已连上桥接服务，开始双向推流", "ok");
  };

  ws.onmessage = (event) => {
    if (typeof event.data === "string") {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "pong" && typeof msg.ts === "number") {
          runtime.rttMs = performance.now() - msg.ts;
          runtime.serverRttMs = msg.server_rtt_ms;
        } else if (msg.type === "status") {
          runtime.tx = Math.max(runtime.tx, msg.tx || 0);
          runtime.rx = msg.rx || 0;
        }
      } catch (err) {
        /* 忽略非 JSON */
      }
      return;
    }

    // 服务器混音后的 PCM（Int16）
    const buf = event.data;
    const i16 = new Int16Array(buf);
    const f32 = new Float32Array(i16.length);
    for (let i = 0; i < i16.length; i++) f32[i] = i16[i] / 0x8000;
    runtime.peerLevel = rms(f32);
    if (runtime.playbackNode) runtime.playbackNode.port.postMessage(f32, [f32.buffer]);
  };

  ws.onclose = (ev) => {
    const wasJoined = runtime.joined;
    runtime.joined = false;
    setState("", "未加入");
    ui.join.disabled = false;
    ui.leave.disabled = true;
    if (wasJoined) log(`连接已关闭（code ${ev.code}）`, "err");
    teardownAudio();
  };

  ws.onerror = () => {
    log("WebSocket 出错：检查 /ws 反代是否正常", "err");
  };
}

async function leave() {
  if (runtime.ws) {
    try {
      runtime.ws.close(1000, "bye");
    } catch (err) {
      /* 忽略 */
    }
    runtime.ws = null;
  }
  runtime.joined = false;
  await teardownAudio();
  setState("", "未加入");
  ui.join.disabled = false;
  ui.leave.disabled = true;
  ui.hint.textContent = "已离开房间。";
  runtime.selfLevel = 0;
  runtime.peerLevel = 0;
  log("已离开房间");
}

/* ── 指标刷新 ───────────────────────────── */
function tick() {
  const rtt = runtime.rttMs;
  ui.rtt.innerHTML = rtt == null ? "—" : rtt.toFixed(1) + "<small>ms</small>";
  ui.tx.textContent = runtime.tx;
  ui.rx.textContent = runtime.rx;

  const est = rtt == null ? null : rtt / 2 + FRAME_MS + 60;
  ui.lat.innerHTML = est == null ? "—" : "≈ " + est.toFixed(0) + "<small>ms</small>";
  ui.lat.className = "v" + (est == null ? "" : est < 120 ? " good" : est < 200 ? " warn" : " bad");

  paintMeter(ui.selfBar, runtime.selfLevel);
  paintMeter(ui.peerBar, runtime.peerLevel);
  ui.selfTxt.textContent = fmtDb(runtime.selfLevel);
  ui.peerTxt.textContent = fmtDb(runtime.peerLevel);
}

/* ── 初始化 UI ──────────────────────────── */
for (const el of [ui.selfBar, ui.peerBar]) {
  el.innerHTML = "";
  for (let i = 0; i < 24; i++) el.appendChild(document.createElement("i"));
}

ui.join.addEventListener("click", join);
ui.leave.addEventListener("click", leave);
ui.mode.addEventListener("change", () => {
  if (runtime.captureNode) runtime.captureNode.port.postMessage({ mode: ui.mode.value });
});
function sendPing() {
  if (runtime.ws && runtime.ws.readyState === WebSocket.OPEN) {
    runtime.ws.send(JSON.stringify({ type: "ping", ts: performance.now() }));
  }
}

setInterval(tick, 200);
setInterval(() => { if (runtime.joined) sendPing(); }, 1000);
tick();
log("页面就绪。点「加入房间」开始。");
