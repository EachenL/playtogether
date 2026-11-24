# 贡献指南：如何新增一个功能

本项目目标是让**多人（包括用 AI 辅助开发、不熟悉底层的人）都能安全地加功能**。
为此，服务器把"功能"做成了**编译期插件**：核心只负责音频热路径（混音），
其余功能都是实现同一个 `Plugin` trait 的独立文件。

## 架构速览（30 秒）

```
server/src/
├── main.rs          核心循环：音频热路径(JT01 混音) + ping + 清理（一般不改）
├── hub.rs           Hub：客户端/房间状态 + broadcast_roster（一般不改）
├── plugin.rs        Plugin trait：唯一的接口（稳定）
└── plugins/         每个功能一个文件
    ├── roster.rs    改名/花名册
    ├── gain.rs      独立音量
    ├── room.rs      房间人数上限/空闲释放
    └── _template.rs ← 复制这个模板
```

**铁律**：插件只处理「控制消息 + 生命周期事件」，**绝不碰音频热路径**，
所以加功能不影响 <20ms 的音频延迟。

## 新增功能的 5 步

### 1. 复制模板

```bash
cp server/src/plugins/_template.rs server/src/plugins/recording.rs
```

### 2. 实现 `Plugin` trait

把模板里的 `MyPlugin` / `"my_plugin"` 改名，实现你需要的那几个钩子：

| 钩子 | 何时调用 | 用途 |
|---|---|---|
| `on_message` | 收到一条控制消息 | 改名(JTNM)、音量(JTGN)… |
| `on_join` | 客户端申请进房（音频首包） | 人数上限等拒绝逻辑 |
| `on_tick` | 每秒一次 | 清理/统计 |

```rust
use jt_core::MAGIC_RECORD; // 假设 core 里定义了你的 magic

impl Plugin for RecordingPlugin {
    fn name(&self) -> &'static str { "recording" }

    fn on_message(&mut self, hub: &mut Hub, src: SocketAddr, buf: &[u8], n: usize) -> bool {
        if buf[..4] != MAGIC_RECORD { return false; }
        // 你的逻辑：读写 hub.clients / hub.socket …
        true // 返回 true = 已处理
    }
}
```

### 3. 注册

在 `server/src/plugins/mod.rs` 里加两处：

```rust
pub mod recording;                    // ① 声明模块

pub fn all() -> Vec<Box<dyn Plugin>> {
    // …
    plugins.push(Box::new(recording::RecordingPlugin)); // ② 加一行
    // …
}
```

### 4. （可选）加 Cargo feature 开关

`server/Cargo.toml`：

```toml
[features]
default = ["room", "roster", "gain", "recording"]
recording = []
```

然后在 `mod.rs` 用 `#[cfg(feature = "recording")]` 包住模块和注册行，
这样就能 `cargo build --no-default-features --features recording` 单独开关它。

### 5. 测试

```bash
cargo test -p jt-server
```

给插件加单测：在插件文件末尾写 `#[cfg(test)] mod tests`，直接构造一个
`Hub`（或 mock），调用 `on_message` 断言行为。

## 一个完整例子（录音插件）

见 `server/src/plugins/_template.rs` 里的注释；完整可运行示例看
`roster.rs`（最简单的 `on_message`）和 `room.rs`（用 `on_join` + `on_tick`）。

## 需要碰 core 的情况

如果新功能需要**新的网络消息**（新 magic），要同步改：

1. `core/src/lib.rs` —— 定义 `MAGIC_XXX` + `make_xxx` / `parse_xxx`
2. 服务器插件 —— 用 `parse_xxx` 处理
3. 客户端/网页桥 —— 用 `make_xxx` 发送

协议格式见 [`docs/protocol.md`](docs/protocol.md)。
