# ADR-0003：网页端经 WebSocket⇄UDP 桥接入（不用 WebRTC）

状态: 已采纳
日期: 2026-11-23

## 背景

浏览器无法直接发 UDP，网页端要进 UDP 房间需要一层适配。可选路线：WebRTC（浏览器原生实时音视频栈）或自建 WebSocket 桥。

## 决策

自建 **`jt_webd.py`（Python asyncio WebSocket 桥）**：每个浏览器 WS 连接在桥上开一个独立 UDP socket（独立源端口），WS 二进制帧承载裸 PCM（480 采样/帧 = 10 ms），桥负责帧 ↔ JT01 包的转换。

## 理由

- **服务器零改动**：桥把每个浏览器用户变成一个普通 UDP 乐手，与桌面端/CLI 同房间 —— 复用了"身份 = UDP 源地址"的语义（ADR-0002）。
- **避开 WebRTC 的复杂度**：SDP 协商、DTLS-SRTP、媒体管线全部不要；音频路径与原生端同构（PCM 进 PCM 出），排查延迟问题时两端可比。
- WebRTC 的编解码与 jitter buffer 均不可控，我们的核心卖点是"未压缩 PCM + 可解释的延迟模型"，WebRTC 会把这层遮住。
- Python asyncio 单文件 ~200 行可维护，WS 桥只做转发与封包，CPU 开销可忽略。

## 代价与后果

- **多一跳**：浏览器 ↔ 桥的 WS（TCP！）会引入队头阻塞与额外缓冲 —— 网页端延迟天然高于原生端（播放侧 60 ms 抖动缓冲 vs 原生 2.7 ms）。
- 帧大小硬编码 10 ms/480 采样，与原生端块大小不一致（靠"最新块"语义掩盖）。
- 桥是常驻服务（systemd），部署多一个组件；Python 运行时依赖（`python3-websockets`）。
- TCP 上的音频丢失 = 尾延迟尖刺，实时性弱于 SCTP/UDP 上的 WebRTC data —— 目前在 60 ms 缓冲下可接受。

## 替代方案（否决）

- **WebRTC DataChannel（unreliable/unordered 模式）**：能拿到类 UDP 语义且支持 SCTP，但信令服务器、STUN/ICE 全套复杂度换来的收益，在当前"服务器在同城、单跳 WS"的假设下不划算。
- **WebTransport（HTTP/3, unreliable datagram）**：浏览器支持面与部署要求（QUIC、TLS 证书）尚不稳定；持续关注，若 WS 尾延迟成为实际痛点可切换 —— 桥的替换成本很低，协议层不用动。
