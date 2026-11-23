# 整体架构设计

状态: 已采纳（描述当前实现）
日期: 2026-11-23
对应代码: `core/` `engine/` `server/` `cli/` `client/` `dist-web/`

---

## 1. 系统总览

playTogether 是全 Rust 的低延迟在线合奏系统，参考 JackTrip / Jamulus：**音频以未压缩 16-bit PCM 直接走 UDP**，用带宽换零编解码延迟。目标端到端单向延迟 < 20 ms（同城）。

拓扑是经典的 **Hub 混音模型**：所有客户端把单声道 PCM 流发给服务器，服务器为每个发送者把「其他所有人的最新块」混音后回发 —— 即"听得见所有人、听不见自己"。

```
                         ┌─────────────────┐
   麦克风 ─┐             │   jt-server     │             ┌─ 扬声器
           ├─ 桌面端 ────▶│   混音 Hub      │◀──── 桌面端 ─┤
   浏览器 ─┴─ 网页版 ────▶│   (UDP 9000)    │◀──── 网页版 ─┴─ 扬声器
                         └─────────────────┘
```

三个客户端形态进同一个房间：

| 形态 | 实现 | 音频栈 |
|---|---|---|
| CLI (`cli/`) | 命令行，500 ms 打印一次状态 | cpal 直连 |
| 桌面端 (`client/`) | Tauri GUI，含独立"测延迟"探针 | cpal，由 jt-engine 封装 |
| 网页版 (`dist-web/`) | 浏览器 AudioWorklet + WebSocket | WebAudio |

三种形态的**共同底座是 jt-core 定义的线协议**（见 [protocol.md](protocol.md)），所以服务器不区分客户端类型 —— 网页用户由 `jt_webd.py` 桥接成一条普通 UDP 流。

## 2. Crate 划分与 workspace 拓扑

```
Cargo.toml          # workspace: core, engine, server, cli
core/               # jt-core：线协议 + 混音内核，std-only 零依赖
engine/             # jt-engine：无头客户端会话（cpal + UDP + 抖动缓冲）
server/             # jt-server：混音 Hub
cli/                # jt-cli：薄壳，循环打印 session 状态
client/src-tauri/   # 独立 workspace（自声明 [workspace]）
```

关键决策：**Tauri 客户端刻意不在主 workspace 里**。Tauri 依赖树庞大，独立出去后音频核心 / 服务器 / CLI 的编译不被拖慢。`client` 通过路径依赖引用 `jt-engine`。

依赖方向（单向，无环）：

```
jt-core  ◀── jt-engine ◀── jt-cli
   ▲             ▲
   │             └────────── client (Tauri, 独立 workspace)
   └── jt-server
```

## 3. jt-core：协议与混音内核（`core/src/lib.rs`）

刻意保持 **std-only 零依赖**，server 与 client 共用而不引入音频/GUI 依赖。提供：

- `AudioBlock`：`{seq: u32, samples: Vec<i16>}`，序列化为 `JT01` 帧（16 字节头 + PCM，小端）。序列号在线上存在，但当前原生实现**未用于重排** —— 留作协议升级空间。
- `make_ping` / `parse_ping`：`JTPN` 12 字节 ping/RTT 帧。
- `mix_blocks`：混音内核。i32 累加 + `clamp` 饱和；块长可不等，输出取最长块长度。
- `client_id` 字段在协议里存在但**当前恒为 0**，客户端身份实际由 UDP 源地址决定（网页桥为每个连接开独立 UDP socket，正是利用了这一点）。

## 4. jt-engine：无头客户端会话（`engine/src/lib.rs`）

核心线程模型 —— **音频回调绝不碰网络**：

```
mic ──▶ cpal input callback ──▶ mpsc(8块, 满则丢) ──▶ 发送线程 ──▶ UDP ──▶ server
                                                                    │
speaker ◀── cpal output callback ◀── SampleFifo(抖动缓冲) ◀── 接收线程 ◀──┘
```

- **采集→网络**：cpal 回调里 `try_send` 进容量 8 的 bounded channel，满了就丢整块 —— 实时音频的正确语义。专职发送线程 `recv_timeout(20ms)` 取块、编 JT01、发 UDP，并每 1 s 发一次 JTPN 测 RTT。
- **网络→播放**：接收线程 20 ms 超时轮询 socket，解析出 PCM push 进 `SampleFifo`；输出回调逐采样 `pop`。
- **`SampleFifo`（抖动缓冲）**：`Mutex<VecDeque<i16>>`。
  - `TARGET_DEPTH = 128` 采样（@48 kHz ≈ 2.7 ms）：低于此深度 `pop` 返回静音 —— 播放前先攒够，平滑开局抖动。
  - `MAX_DEPTH = 4096` 采样（≈85 ms）：push 超限时从头部丢最老数据 —— 网络短暂卡顿时延迟有上界。
- **格式归一**：所有输入格式（I16/U16/F32、任意声道数）统一塌缩为**单声道 i16**；输出时单采样复制到所有声道。48 kHz 是规范采样率，但引擎按设备实际采样率计算延迟模型。
- **延迟模型**（`est_one_way_ms`，模型估算而非硬件实测）：

  ```
  单向延迟 ≈ RTT/2（网络） + 2×块时长（采集+播放） + 抖动目标（TARGET_DEPTH）
  ```

  块时长按输入流支持的最小 buffer size（未知则保守取 256 帧）。用于判断是否在 20 ms 排练预算内。
- **生命周期**：`ClientSession` Drop 时置 stop 标志、drop 掉 sender（让发送线程观察到断链）、join 全部线程。cpal Stream 存在字段里保活。

## 5. jt-server：混音 Hub（`server/src/main.rs`）

**单线程 UDP 事件循环**，无任何 per-client 发送定时器 —— 混音输出由客户端自己的发送节奏驱动：

```
recv_from →
  ├─ JTPN? 原样 echo 回发送者（测 RTT 用）
  └─ JT01?
       1. clients[src].last_block = 本块   （HashMap<SocketAddr, ClientState>)
       2. blocks = 所有客户端的 last_block，排除发送者（--echo 模式则包含）
       3. mix_blocks(blocks) → JT01 → send_to(发送者)
```

- **客户端识别**：`HashMap<SocketAddr, ...>`，键是 UDP 源地址。10 s 未见即剪除（每 1 s 检查一次）。
- **语义**：每个客户端每发一块，就收到一块"其他人此刻最新音频"的混音。这隐含了时间对齐是**松弛的** —— 某人网络抖动，其他人听到的他的声音会"变慢/卡顿"（最新块被重复消费），而不是全局对齐。
- `--echo`：把发送者也混进去，单机回环测试用。

## 6. Tauri 桌面客户端（`client/src-tauri/src/lib.rs`）

薄封装，一个关键约束：**`cpal::Stream` 在部分平台（尤其 macOS CoreAudio）非 `Send`/`Sync`**。因此：

- 单一 **engine 线程**终身持有 `ClientSession`（Start / Stop / Status 命令经 `mpsc` 传入，channel 是 `Send`），全程无 `unsafe`。
- `ping_server` 命令是**独立的一次性 JTPN 探针**（新 socket、1.5 s 超时）—— 不碰音频设备，没麦克风权限也能先测网络延迟。

## 7. 网页版（`dist-web/`）

### 7.1 桥接进程 `jt_webd.py`（WebSocket ⇄ UDP）

**每个浏览器连接 = 一个独立 UDP socket（独立源端口）= 一个独立乐手**。这是网页端能和桌面端同房间的全部原理 —— 服务器只看 UDP 源地址。

- 浏览器 → 桥：WebSocket 二进制帧，**每帧固定 480 采样（10 ms @48 kHz）= 960 字节 i16 PCM**，帧化由浏览器完成，桥只做攒字节→拆帧→套 JT01 头（带自己的 seq）→ UDP。
- 桥 → 浏览器：剥掉 JT01 头，payload 原样经 WS 推回。
- JSON 控制消息：`{type:"ping"}`（测浏览器↔桥 RTT，顺带触发桥↔服务器 JTPN）、每秒一条 `{type:"status", rtt_ms, tx, rx, frame_ms}`。
- WS 开 `compression=None`：音频不压缩，省 CPU 和延迟。
- 环境变量配置：`JT_WEB_HOST/PORT`（默认 127.0.0.1:9100，nginx 反代）、`JT_SERVER_HOST/PORT`（默认 127.0.0.1:9000）。

### 7.2 浏览器端（`web/jam.js` + worklets）

- `AudioContext({sampleRate: 48000, latencyHint: "interactive"})`。
- **采集 worklet**：mic 或内置 440 Hz 测试音（约 -16 dBFS，无麦克风机器验证链路用），攒 480 采样 post 给主线程。
- **播放 worklet**：2 秒环形缓冲，`targetFill = 60 ms` 攒够才开播；溢出（>800 ms）时读指针跳回 targetFill —— 丢最老。
- 注意网页抖动缓冲（60 ms）远大于原生（~2.7 ms）—— 这是浏览器音频栈的约束，也是网页端延迟预算的主要开销项。

## 8. 部署形态（`deploy/`）

- `jt-server.service`：systemd 常驻，`Restart=always`，UDP 9000。
- `jt-webd.service`：桥常驻 127.0.0.1:9100。
- `nginx-playtogether.conf`：站点 + WS 反代。
- `site/`：静态产品页，含实时状态接口。

## 9. 已知限制与设计债（未来开发方向）

按影响排序：

1. **服务器时间对齐是"最新块"语义**：没有按 seq/时间戳对齐各路流，混音相位松弛。Jamulus 式的 per-client 播放缓冲对齐是下一步的核心工作。
2. **seq 未用于重排/丢包掩盖**：`SampleFifo` 是纯样本队列（无序号感知），乱序包直接按到达顺序播放。协议已预留 seq，升级不需改协议。
3. **单声道**：协议和引擎全链路 mono。
4. **无认证/加密**：任何知道地址的人都能进房间；UDP 明文。
5. **无房间概念**：一台服务器 = 一个全局房间。
6. **服务器单线程**：每包全量重混所有客户端（O(n²) 收发包/块），人数多时是瓶颈；`HashMap` 每包迭代也无缓存友好性。
7. **无音量控制/电平表**：混音是纯饱和求和，人多了削波严重（10 人满电平必然 clip）。
8. **客户端静音判定粗**：10 s 未见才剪除，中途掉线的人会以"最后一块"被混很久。
9. **`client_id` 恒 0**：将来 NAT 重绑定（源端口变化）会被当成新乐手，旧条目等 10 s 才过期。
10. **网页帧固定 10 ms**：480 采样/帧是硬编码，与原生块大小（由设备决定）不一致，两边时基靠服务器"最新块"语义掩盖。

## 10. 相关文档

- [protocol.md](protocol.md) —— JT01 / JTPN 线协议规范
- [../adr/0001-uncompressed-pcm-over-udp.md](../adr/0001-uncompressed-pcm-over-udp.md)
- [../adr/0002-hub-mixing-topology.md](../adr/0002-hub-mixing-topology.md)
- [../adr/0003-web-via-ws-udp-bridge.md](../adr/0003-web-via-ws-udp-bridge.md)
