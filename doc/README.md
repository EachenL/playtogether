# doc/ — 开发文档

存放后续开发过程中产生的设计文档、决策记录、调研笔记与运维手册。

> 已有的用户向文档（架构总览、部署指南）在仓库根目录 `README.md` 与 `deploy/`，此处不重复，专注于**开发过程**文档。

## 约定

- 文件名用英文小写-连字符（如 `mixing-design.md`），标题可中文。
- 设计决策类文档建议带上日期与状态（`状态: 提案 / 已采纳 / 已废弃`）。
- 需求文档（`requirements/`）建议一份需求一个文件，包含：背景/动机、用户故事、验收标准、优先级，并在实现后链接对应的设计文档或 ADR。

## 目录结构

| 路径 | 放什么 |
|---|---|
| `requirements/` | 需求文档：新功能需求、用户故事、验收标准 |
| `design/` | 模块/协议设计方案（混音内核、抖动缓冲、WebRTC 替代方案等） |
| `notes/` | 调研笔记、实验记录（延迟测量数据、不同平台对比） |
| `adr/` | 架构决策记录（Architecture Decision Records，一决策一文件） |
| `ops/` | 运维排障手册（服务器部署踩坑、系统调优） |

## 索引

新增文档后在此登记：

| 文档 | 说明 |
|---|---|
| [requirements/no-fixed-ip-rehearsal.md](requirements/no-fixed-ip-rehearsal.md) | 需求：4–6 人无需固定 IP 的远程排练（房主托管模式 + 打洞，含方案对比与分期路线） |
| [design/architecture.md](design/architecture.md) | 整体架构：系统总览、crate 划分、线程模型、各组件设计、已知限制与设计债 |
| [design/protocol.md](design/protocol.md) | 线协议规范：JT01 音频帧、JTPN ping、WebSocket 侧协议 |
| [design/host-mode.md](design/host-mode.md) | 方案 C：房主托管模式 —— 房主电脑即服务器（含 UPnP/STUN 三档兜底、通俗解释） |
| [design/p2p-mesh.md](design/p2p-mesh.md) | 方案 D：P2P 全互联 mesh —— 两两直连、本地混音、每流独立抖动缓冲（含通俗解释） |
| [adr/0001-uncompressed-pcm-over-udp.md](adr/0001-uncompressed-pcm-over-udp.md) | 音频走未压缩 PCM over UDP |
| [adr/0002-hub-mixing-topology.md](adr/0002-hub-mixing-topology.md) | Hub 混音拓扑，服务器输出由客户端节奏驱动 |
| [adr/0003-web-via-ws-udp-bridge.md](adr/0003-web-via-ws-udp-bridge.md) | 网页端经 WebSocket⇄UDP 桥接入（否决 WebRTC） |
