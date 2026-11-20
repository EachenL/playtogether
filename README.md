# playTogether

[![CI](https://github.com/EachenL/playtogether/actions/workflows/ci.yml/badge.svg)](https://github.com/EachenL/playtogether/actions/workflows/ci.yml)

**低延迟在线合奏 / 远程排练系统** —— 全 Rust 实现，参考 [JackTrip](https://github.com/jacktrip/jacktrip) 与 [Jamulus](https://github.com/jamulussoftware/jamulus) 的思路：**不压缩 PCM 走 UDP**，把延迟压到「能听出同一个拍子」的量级。

目标：端到端单向延迟 **< 20 ms**（同城），并提供三端接入 —— **桌面客户端、命令行客户端、网页版**，三者进同一个房间。

```
                         ┌─────────────────┐
   麦克风 ─┐             │   jt-server     │             ┌─ 扬声器
           ├─ 桌面端 ────▶│   混音 Hub      │◀──── 桌面端 ─┤
   浏览器 ─┴─ 网页版 ────▶│   (UDP 9000)    │◀──── 网页版 ─┴─ 扬声器
                         └─────────────────┘
```

---

## 目录结构

| 路径 | 说明 |
|---|---|
| `core/` | 协议层：`JT01` UDP 帧（16-bit PCM + 序列号）、`JTPN` ping/RTT、混音内核 |
| `engine/` | 无头客户端会话：cpal 采集/播放 + UDP 收发 + 抖动缓冲 + 延迟估算 |
| `server/` | 播控服务器：多客户端上行 → 混音（排除发送者自己）→ 回发 + ping echo |
| `cli/` | 命令行客户端，实时打印 RTT / 抖动深度 / 单向延迟估算 |
| `client/` | **Tauri 桌面客户端**（macOS CoreAudio / Windows WASAPI），含「测延迟」按钮 |
| `dist-web/` | **网页版**：`jt_webd.py`（WebSocket ⇄ UDP 桥）+ 浏览器端 AudioWorklet 客户端 |
| `site/` | 产品主站（静态页，含实时状态接口） |
| `deploy/` | systemd 单元、nginx 配置、状态采集脚本、站点部署脚本 |

---

## 快速开始

### 1. 服务器

```bash
cargo build --release -p jt-server
./target/release/jt-server --port 9000
```

生产部署用 `deploy/jt-server.service`（systemd，`Restart=always`）。

### 2. 命令行客户端（最快验证延迟）

```bash
cargo build --release -p jt-cli
./target/release/jt-cli <server-ip>:9000
```

输出会实时显示：

```
采样率  48000 Hz   块 256 frames (5.33 ms)
RTT      0.9 ms   抖动缓冲 2 blocks
单向延迟估算  6.4 ms  目标 < 20 ms ✓
```

> 测真实延迟前**务必关掉代理**（Surge / Clash 之类会把 UDP 绕到境外，凭空多 30–50 ms）。

### 3. 桌面客户端

```bash
cargo build --release -p jt-client
# 产物：client/src-tauri/target/release/
```

界面上填服务器地址 → 先点「**测延迟**」（只发 ping，不需要麦克风权限）→ 再点「连接」。

### 4. 网页版（免安装）

```bash
# 服务器上：装依赖并启动桥接服务
sudo apt install -y python3-websockets
sudo install -m 755 dist-web/jt_webd.py /usr/local/bin/
sudo cp deploy/jt-webd.service /etc/systemd/system/
sudo systemctl enable --now jt-webd     # 监听 127.0.0.1:9100

# 反向代理（见 deploy/nginx-playtogether.conf）
#   location /ws { proxy_pass http://127.0.0.1:9100; ... }
```

然后把 `dist-web/web/` 放到站点根目录下的 `/web/`，浏览器打开即可加入房间。

---

## 延迟预算

端到端 ≈ 网络单向 + 采集缓冲 + 抖动缓冲 + 播放缓冲（无编解码延迟）。

| 环节 | 典型值 | 是否可控 |
|---|---|---|
| 网络单向（同城） | 0.5 – 1.0 ms | 取决于线路 |
| 采集缓冲（48 kHz / 256 帧） | ≈ 5.3 ms | 可调 |
| 抖动缓冲（2 块起步） | ≈ 5.3 ms | 可调，按丢包自适应 |
| 播放缓冲 | ≈ 5.3 ms | 可调 |
| 编解码 | **0 ms** | 不压缩 PCM |

**桌面端 / 命令行**：同城可做到个位数到十几毫秒。
**网页版**：受限于 TCP + 浏览器音频缓冲，单向约 **60–120 ms**，适合听排练、跟节奏，不适合严格 &lt;20 ms 的合奏 —— 这是协议栈决定的，不是实现问题。

---

## 协议

```
音频帧（UDP）
  [0..4)   "JT01"
  [4..8)   client_id  u32
  [8..12)  seq        u32    ← 抖动缓冲按它重排
  [12..16) num_samples u32
  [16..)   samples     i16 × num_samples   (little-endian, 48 kHz mono)

延迟探测（UDP）
  [0..4)   "JTPN"
  [4..12)  timestamp_ms u64    ← 服务器原样回显，客户端算 RTT

网页版（WebSocket 二进制）
  48 kHz 单声道 16-bit PCM，每帧 480 采样（10 ms = 960 字节）
  文本帧用于控制：{"type":"ping","ts":...} → {"type":"pong",...}
```

服务器的混音规则：**把所有别人的最近一帧混音后发给请求者，排除请求者自己**（听得到乐队，听不到自己的回声）。

---

## 测试

```bash
cargo test                 # core + engine 单元测试
cargo test -p jt-server    # 集成测试：spawn 真实服务器，验证音频回环 + 双客户端混音
```

CI（`.github/workflows/ci.yml`）在 ubuntu / macOS / Windows 三平台构建与测试，并检查 Tauri 客户端编译。

---

## 双平台

| 平台 | 音频后端 | 状态 |
|---|---|---|
| macOS | CoreAudio | ✅ 编译 + 运行验证 |
| Windows | WASAPI (cpal) | ✅ 交叉编译验证（CI 实机构建） |
| Linux | ALSA (cpal) | ✅ 服务器端长期运行验证 |

---

## License

MIT
