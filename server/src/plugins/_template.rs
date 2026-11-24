//! 新插件模板 —— 复制本文件 → 改名 → 实现 Plugin → 在 `mod.rs` 里注册。
//!
//! 步骤：
//!   1. 复制本文件为 `plugins/你的功能.rs`
//!   2. 把 `MyPlugin` / `"my_plugin"` 改成你的名字
//!   3. 按需实现 `on_message` / `on_join` / `on_tick`
//!   4. 在 `plugins/mod.rs` 里：`pub mod 你的功能;` + `all()` 里加一行
//!   5. （可选）在 `server/Cargo.toml` 加 feature 开关

use std::net::SocketAddr;

use crate::hub::Hub;
use crate::plugin::Plugin;

#[allow(dead_code)] // 模板：复制改名后才被使用
pub struct MyPlugin;

impl Plugin for MyPlugin {
    fn name(&self) -> &'static str {
        "my_plugin"
    }

    // 可选：处理控制消息（改名/音量/…）。返回 true 表示已处理。
    // 用 `jt_core::parse_xxx(&buf[..n])` 解析你的 magic，再更新 `hub` 状态。
    fn on_message(&mut self, _hub: &mut Hub, _src: SocketAddr, _buf: &[u8], _n: usize) -> bool {
        false
    }

    // 可选：客户端加入房间前检查。返回 Some(拒绝包) 表示拒绝。
    // fn on_join(&mut self, _hub: &mut Hub, _room: u32) -> Option<Vec<u8>> { None }

    // 可选：每秒一次（清理/统计等）。
    // fn on_tick(&mut self, _hub: &mut Hub) {}
}
