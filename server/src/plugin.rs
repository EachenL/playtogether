//! 插件契约 —— 整个系统的唯一接口。
//!
//! 每个"功能"实现 [`Plugin`]，并在 `plugins::all()` 里注册一行。
//! 插件只处理**控制消息与生命周期事件**，不碰音频热路径（JT01 混音），
//! 因此不影响 <20ms 的音频延迟。
//!
//! # 新增一个功能
//! 1. 复制 `plugins/_template.rs` → `plugins/你的功能.rs`
//! 2. 实现 `Plugin`（按需覆写钩子）
//! 3. 在 `plugins/mod.rs` 的 `all()` 里加一行注册
//! 4. （可选）在 `server/Cargo.toml` 加一个 feature 开关

use std::net::SocketAddr;

use crate::hub::Hub;

pub trait Plugin: Send + Sync {
    /// 插件唯一名称（用于日志/测试）。
    fn name(&self) -> &'static str;

    /// 处理一条控制消息（改名/音量等）。返回 `true` 表示已处理，
    /// 核心将不再把它当作音频帧。
    fn on_message(&mut self, _hub: &mut Hub, _src: SocketAddr, _buf: &[u8], _n: usize) -> bool {
        false
    }

    /// 新客户端申请加入房间（收到音频首包）。返回 `Some(拒绝包)` 表示拒绝。
    fn on_join(&mut self, _hub: &mut Hub, _room: u32) -> Option<Vec<u8>> {
        None
    }

    /// 每秒一次（房间清理等）。
    fn on_tick(&mut self, _hub: &mut Hub) {}
}
