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

/* ── 预览模式 ───────────────────────────── */
const PREVIEW = !bridge;
const previewIdle = new URLSearchParams(location.search).get("state") === "idle";

function previewPing() {
  setState("busy", "测延迟…");
  setHint("正在向 " + els.server.value + " 发 ping…");
  setTimeout(() => {
    paintMetrics({ rtt: 0.9, oneWay: 6.4 });
    setState("on", "网络正常");
    setHint("RTT 0.9 ms · 单向约 6.4 ms · 这块不需要麦克风", "ok");
    setMeter(els.mic, true);
    setMeter(els.peer, true);
  }, 320);
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
  if (PREVIEW) return previewConnect();
  setState("busy", "连接中…");
  els.start.disabled = true;
  try {
    const msg = await bridge("start_session", { server: els.server.value });
    const bad = /fail|error|invalid/i.test(msg || "");
    setState(bad ? "idle" : "on", bad ? "连接失败" : "已连接");
    setHint(msg, bad ? "err" : "ok");
    els.start.disabled = !bad;
  } catch (e) {
    setState("idle", "连接失败");
    setHint(String(e), "err");
    els.start.disabled = false;
  }
});

els.stop.addEventListener("click", async () => {
  if (PREVIEW) {
    setState("idle", "未连接");
    setHint("已断开。", "");
    setMeter(els.mic, false);
    setMeter(els.peer, false);
    els.start.disabled = false;
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
  setMeter(els.mic, false);
  setMeter(els.peer, false);
});

/* ── 初始化 ─────────────────────────────── */
buildMeter(els.mic, IDLE_MIC);
buildMeter(els.peer, IDLE_PEER);
els.stop.disabled = true;

if (PREVIEW) {
  if (!previewIdle) setTimeout(previewConnect, 60);
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
