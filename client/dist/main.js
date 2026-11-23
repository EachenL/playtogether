// playTogether 桌面客户端前端
// Tauri v2 会在 withGlobalTauri 下注入 window.__TAURI__。
// 没有桥的时候（例如直接用浏览器打开本页面），进入「预览模式」，
// 用一组固定的示例数值渲染界面，方便截图与 UI 调试。

const bridge =
  window.__TAURI__?.core?.invoke ||
  window.__TAURI__?.tauri?.invoke ||
  window.__TAURI__?.invoke ||
  null;

const $ = (id) => document.getElementById(id);
const els = {
  server: $("server"),
  room: $("room"),
  name: $("name"),
  members: $("members"),
  memberList: $("member-list"),
  inputDevice: $("input-device"),
  outputDevice: $("output-device"),
  ptt: $("ptt"),
  state: $("link-state"),
  hint: $("hint"),
  start: $("start"),
  stop: $("stop"),
  ping: $("ping"),
  rtt: $("m-rtt"),
  ow: $("m-ow"),
  jit: $("m-jit"),
  target: $("target"),
  mic: $("mic"),
  peer: $("peer"),
};

const BAR_COUNT = 18;

function buildMeter(el, heights) {
  el.innerHTML = "";
  for (let i = 0; i < BAR_COUNT; i++) {
    const bar = document.createElement("i");
    const h = heights[i % heights.length];
    bar.style.height = Math.max(8, h) + "%";
    bar.style.background =
      h > 88 ? "linear-gradient(180deg,#3ddc97,#22d3ee)"
      : h > 60 ? "linear-gradient(180deg,#7c5cff,#5a7bff)"
      : "rgba(124,92,255,.55)";
    el.appendChild(bar);
  }
}

const IDLE_MIC = [6, 6, 6, 6, 6, 6, 6, 6];
const IDLE_PEER = [6, 6, 6, 6, 6, 6, 6, 6];
const LIVE_MIC = [22, 58, 92, 46, 28, 74, 96, 38, 18, 52, 84, 34, 24, 62, 88, 42, 26, 48];
const LIVE_PEER = [34, 66, 48, 88, 56, 30, 72, 94, 52, 26, 60, 82, 38, 70, 44, 90, 54, 32];

function setMeter(el, live) {
  const src = live ? (el === els.mic ? LIVE_MIC : LIVE_PEER) : (el === els.mic ? IDLE_MIC : IDLE_PEER);
  [...el.children].forEach((bar, i) => {
    const h = src[i % src.length];
    bar.style.height = Math.max(8, h) + "%";
  });
}

function setState(kind, text) {
  els.state.className = "chip" + (kind === "on" ? " on" : kind === "busy" ? " busy" : "");
  els.state.textContent = text;
}

function setHint(text, cls) {
  els.hint.textContent = text;
  els.hint.className = "hint" + (cls ? " " + cls : "");
}

function fmtMs(v) {
  return v == null ? "—" : v.toFixed(v < 10 ? 1 : 0) + '<small>ms</small>';
}

function gradeMs(el, ms) {
  el.classList.remove("good", "warn", "bad");
  if (ms == null) return;
  el.classList.add(ms < 15 ? "good" : ms < 25 ? "warn" : "bad");
}

function paintMetrics({ rtt, oneWay, jitter }) {
  if (rtt != null) {
    els.rtt.innerHTML = fmtMs(rtt);
    gradeMs(els.rtt, rtt);
  }
  if (oneWay != null) {
    els.ow.innerHTML = fmtMs(oneWay);
    gradeMs(els.ow, oneWay);
  }
  if (jitter != null) els.jit.innerHTML = jitter;

  if (oneWay != null) {
    els.target.textContent = oneWay < 20 ? "目标 < 20 ms ✓" : "目标 < 20 ms ✗";
    els.target.className = "target " + (oneWay < 20 ? "ok" : "warn");
  }
}

function parseStatus(text) {
  const m = /jitter\s+(\d+)\s+samples.*?RTT\s+([\d.]+)\s*ms.*?one-way\s*~?\s*([\d.]+)\s*ms/i.exec(text || "");
  if (!m) return null;
  return { jitter: m[1], rtt: parseFloat(m[2]), oneWay: parseFloat(m[3]) };
}

function getMode() {
  return document.querySelector('input[name="mode"]:checked')?.value || "rehearsal";
}

async function populateDevices() {
  if (PREVIEW || !bridge) return;
  try {
    const devs = await bridge("list_devices");
    const fill = (sel, list) => {
      sel.innerHTML = '<option value="">默认</option>';
      for (const d of list) {
        const opt = document.createElement("option");
        opt.value = d.name;
        opt.textContent = d.name + (d.is_default ? "（默认）" : "");
        sel.appendChild(opt);
      }
    };
    fill(els.inputDevice, devs.inputs || []);
    fill(els.outputDevice, devs.outputs || []);
  } catch (e) {
    /* ignore */
  }
}

async function populateWebDevices() {
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const fill = (sel, kind) => {
      sel.innerHTML = '<option value="">默认</option>';
      for (const d of devices) {
        if (d.kind === kind && d.deviceId) {
          const opt = document.createElement("option");
          opt.value = d.deviceId;
          opt.textContent = d.label || (kind === "audioinput" ? "麦克风" : "扬声器");
          sel.appendChild(opt);
        }
      }
    };
    fill(els.inputDevice, "audioinput");
    fill(els.outputDevice, "audiooutput");
  } catch (e) {
    /* ignore */
  }
}

function updatePttVisibility() {
  const voice = getMode() === "voice";
  els.ptt.style.display = voice ? "" : "none";
}

function talk(v) {
  if (bridge && !PREVIEW) {
    bridge("set_talking", { talking: v }).catch(() => {});
  } else {
    web.muted = !v;
  }
}

async function pollRoster() {
  if (PREVIEW || !bridge) return;
  try {
    const r = await bridge("get_roster");
    renderRoster(r);
  } catch (e) {
    /* ignore */
  }
}

function renderRoster(r) {
  const own = r.own_id;
  const members = r.members || [];
  els.members.style.display = members.length ? "" : "none";
  els.memberList.innerHTML = "";
  for (const m of members) {
    const row = document.createElement("div");
    row.className = "member";
    const label = document.createElement("span");
    label.className = "member-name";
    label.textContent = m.name + (m.id === own ? "（我）" : "");
    row.appendChild(label);
    if (m.id !== own) {
      const slider = document.createElement("input");
      slider.type = "range";
      slider.min = "0";
      slider.max = "200";
      slider.value = "100";
      slider.className = "gain";
      slider.title = m.name + " 音量";
      slider.addEventListener("input", () => {
        const gain = Number(slider.value) / 100;
        if (bridge && !PREVIEW) {
          bridge("set_gain", { peer: m.id, gain }).catch(() => {});
        } else if (web.ws && web.ws.readyState === WebSocket.OPEN) {
          web.ws.send(JSON.stringify({ type: "gain", peer: m.id, gain }));
        }
      });
      row.appendChild(slider);
    }
    els.memberList.appendChild(row);
  }
}

/* ── 浏览器音频（WebSocket + Web Audio）── */
const FRAME_SAMPLES = 480;
const web = {
  ws: null, ctx: null, stream: null, capture: null, playback: null, mute: null,
  joined: false, rttMs: null, pingTimer: null, muted: false, ownId: null,
};

function wsUrl() {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${location.host}/ws`;
}

async function startWebAudio(room, name, mode) {
  setState("busy", "连接中…");
  els.start.disabled = true;
  try {
    const ctx = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
    web.ctx = ctx;
    await ctx.resume();

    // 输出设备（浏览器 setSinkId，若支持）
    if (els.outputDevice.value && typeof ctx.setSinkId === "function") {
      try { await ctx.setSinkId(els.outputDevice.value); } catch (e) { /* ignore */ }
    }

    await ctx.audioWorklet.addModule("worklets/capture-worklet.js");
    await ctx.audioWorklet.addModule("worklets/playback-worklet.js");

    const playback = new AudioWorkletNode(ctx, "playback-processor", {
      numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [2],
      processorOptions: { targetMs: 60 },
    });
    playback.connect(ctx.destination);
    web.playback = playback;

    const capture = new AudioWorkletNode(ctx, "capture-processor", {
      numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
      channelCount: 1, channelCountMode: "explicit",
      processorOptions: { frameSamples: FRAME_SAMPLES, mode: "mic" },
    });
    capture.port.postMessage({ mode: "mic" });

    const audioConstraints = {
      // 语音模式开启浏览器内置 AEC（WebRTC AEC3），排练模式关（保音乐保真）
      echoCancellation: mode === "voice",
      noiseSuppression: false,
      autoGainControl: false,
      channelCount: 1,
    };
    if (els.inputDevice.value) audioConstraints.deviceId = { exact: els.inputDevice.value };
    web.stream = await navigator.mediaDevices.getUserMedia({
      audio: audioConstraints,
      video: false,
    });
    const src = ctx.createMediaStreamSource(web.stream);
    src.connect(capture);

    const mute = ctx.createGain();
    mute.gain.value = 0;
    capture.connect(mute);
    mute.connect(ctx.destination);
    web.capture = capture;
    web.mute = mute;

    capture.port.onmessage = (event) => {
      if (web.muted) return; // 静音时不发送
      const f32 = event.data;
      const i16 = new Int16Array(f32.length);
      for (let i = 0; i < f32.length; i++) {
        const s = Math.max(-1, Math.min(1, f32[i]));
        i16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
      }
      if (web.ws && web.ws.readyState === WebSocket.OPEN) web.ws.send(i16.buffer);
    };

    const ws = new WebSocket(wsUrl());
    ws.binaryType = "arraybuffer";
    web.ws = ws;

    ws.onopen = () => {
      web.joined = true;
      ws.send(JSON.stringify({ type: "room", room }));
      ws.send(JSON.stringify({ type: "name", name }));
      setState("on", "已加入");
      setHint(`已进入房间 ${room}（浏览器 · ${mode === "voice" ? "语音+AEC" : "排练"}）`, "ok");
      els.stop.disabled = false;
    };

    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        try {
          const msg = JSON.parse(event.data);
          if (msg.type === "pong" && typeof msg.ts === "number") {
            web.rttMs = performance.now() - msg.ts;
            paintMetrics({ rtt: web.rttMs, oneWay: web.rttMs / 2 + FRAME_SAMPLES / 48 + 60 });
          } else if (msg.type === "welcome") {
            web.ownId = msg.id;
          } else if (msg.type === "roster") {
            renderRoster({
              own_id: web.ownId,
              members: (msg.members || []).map(([id, name]) => ({ id, name })),
            });
          }
        } catch (e) { /* ignore */ }
        return;
      }
      const i16 = new Int16Array(event.data);
      const f32 = new Float32Array(i16.length);
      for (let i = 0; i < i16.length; i++) f32[i] = i16[i] / 0x8000;
      if (web.playback) web.playback.port.postMessage(f32, [f32.buffer]);
    };

    ws.onclose = () => {
      web.joined = false;
      setState("idle", "未连接");
      setHint("连接已关闭", "");
      els.start.disabled = false;
      els.stop.disabled = true;
      stopWebAudio();
    };

    ws.onerror = () => setHint("WebSocket 出错（检查 /ws 反代）", "err");

    web.pingTimer = setInterval(() => {
      if (web.ws && web.ws.readyState === WebSocket.OPEN) {
        web.ws.send(JSON.stringify({ type: "ping", ts: performance.now() }));
      }
    }, 1000);
  } catch (err) {
    setState("idle", "未连接");
    setHint("音频初始化失败：" + (err && err.message ? err.message : err), "err");
    els.start.disabled = false;
    stopWebAudio();
  }
}

async function stopWebAudio() {
  try {
    if (web.pingTimer) clearInterval(web.pingTimer);
    web.pingTimer = null;
    if (web.capture) web.capture.disconnect();
    if (web.mute) web.mute.disconnect();
    if (web.playback) web.playback.disconnect();
    if (web.stream) web.stream.getTracks().forEach((t) => t.stop());
    if (web.ctx && web.ctx.state !== "closed") await web.ctx.close();
  } catch (e) { /* ignore */ }
  web.ctx = null; web.stream = null; web.capture = null; web.playback = null; web.mute = null;
  if (web.ws) { try { web.ws.close(); } catch (e) {} web.ws = null; }
  web.joined = false;
}

/* ── 预览模式 ───────────────────────────── */
const PREVIEW = !bridge;
const previewIdle = new URLSearchParams(location.search).get("state") === "idle";

function previewPing() {
  setHint("浏览器模式：点「连接」进房间后自动测延迟", "");
}

function previewConnect() {
  setState("busy", "连接中…");
  setHint("正在建立音频流…");
  setTimeout(() => {
    paintMetrics({ rtt: 0.9, oneWay: 6.4, jitter: 512 });
    setState("on", "已连接");
    setHint("已连上 " + els.server.value + " · 乐队 2 人", "ok");
    setMeter(els.mic, true);
    setMeter(els.peer, true);
    els.start.disabled = true;
    els.stop.disabled = false;
  }, 420);
}

/* ── 事件 ───────────────────────────────── */
els.ping.addEventListener("click", async () => {
  if (PREVIEW) return previewPing();
  setState("busy", "测延迟…");
  try {
    const msg = await bridge("ping_server", { server: els.server.value });
    const m = /RTT\s+([\d.]+)\s*ms.*?单向约\s*([\d.]+)\s*ms/i.exec(msg || "");
    if (m) {
      paintMetrics({ rtt: parseFloat(m[1]), oneWay: parseFloat(m[2]) });
      setHint(msg, "ok");
      setState("on", "网络正常");
    } else {
      setHint(msg || "已发出 ping", "ok");
      setState("idle", "未连接");
    }
  } catch (e) {
    setHint("测延迟失败：" + e, "err");
    setState("idle", "未连接");
  }
});

els.start.addEventListener("click", async () => {
  if (PREVIEW) {
    const room = Number(els.room.value) || 0;
    return startWebAudio(room, els.name.value, getMode());
  }
  setState("busy", "连接中…");
  els.start.disabled = true;
  try {
    const msg = await bridge("start_session", {
      server: els.server.value,
      room: els.room.value,
      mode: getMode(),
      inputDevice: els.inputDevice.value || null,
      outputDevice: els.outputDevice.value || null,
    });
    const bad = /fail|error|invalid/i.test(msg || "");
    setState(bad ? "idle" : "on", bad ? "连接失败" : "已连接");
    setHint(msg, bad ? "err" : "ok");
    els.start.disabled = !bad;
    els.ptt.disabled = bad;
    if (!bad) {
      bridge("set_name", { name: els.name.value }).catch(() => {});
      pollRoster();
    }
  } catch (e) {
    setState("idle", "连接失败");
    setHint(String(e), "err");
    els.start.disabled = false;
  }
});

els.stop.addEventListener("click", async () => {
  if (PREVIEW) {
    stopWebAudio();
    setState("idle", "未连接");
    setHint("已断开。", "");
    setMeter(els.mic, false);
    setMeter(els.peer, false);
    els.start.disabled = false;
    els.stop.disabled = true;
    return;
  }
  try {
    await bridge("stop_session");
  } catch (e) {
    /* ignore */
  }
  setState("idle", "未连接");
  setHint("已断开。", "");
  els.start.disabled = false;
  els.stop.disabled = true;
  els.ptt.disabled = true;
  setMeter(els.mic, false);
  setMeter(els.peer, false);
});

/* ── 初始化 ─────────────────────────────── */
buildMeter(els.mic, IDLE_MIC);
buildMeter(els.peer, IDLE_PEER);
els.stop.disabled = true;

populateDevices();
if (PREVIEW) populateWebDevices();
updatePttVisibility();

let muted = false;
els.ptt.addEventListener("click", () => {
  muted = !muted;
  talk(!muted);
  els.ptt.textContent = muted ? "取消静音" : "静音";
});
document.querySelectorAll('input[name="mode"]').forEach((r) =>
  r.addEventListener("change", updatePttVisibility)
);

els.name.addEventListener("change", () => {
  if (!PREVIEW && bridge) bridge("set_name", { name: els.name.value }).catch(() => {});
});

setInterval(pollRoster, 1500);

if (PREVIEW) {
  setHint("浏览器模式：点「连接」进房间，麦克风采集 + 实时对讲", "");
} else {
  const poll = async () => {
    try {
      const text = await bridge("session_status");
      const s = parseStatus(text);
      if (s) {
        paintMetrics(s);
        setState("on", "已连接");
      } else if (/idle/i.test(text || "")) {
        setState("idle", "未连接");
      }
    } catch (e) {
      /* ignore */
    }
  };
  setInterval(poll, 1000);
  poll();
}
