//! 插件注册表 —— 编译期组合点。
//!
//! 新增功能在这里：① `pub mod 你的功能;` ② 在 [`all`] 里加一行注册。

pub mod _template;

#[cfg(feature = "roster")]
pub mod roster;
#[cfg(feature = "gain")]
pub mod gain;
#[cfg(feature = "room")]
pub mod room;
#[cfg(feature = "music")]
pub mod music;
#[cfg(feature = "recording")]
pub mod recording;
#[cfg(feature = "playback")]
pub mod playback;

use crate::plugin::Plugin;

/// 根据启用的 Cargo feature，编译期组合出已注册的插件列表。
pub fn all() -> Vec<Box<dyn Plugin>> {
    #[allow(unused_mut)] // 关闭所有 feature 时无插件，mut 才多余
    let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();
    #[cfg(feature = "roster")]
    plugins.push(Box::new(roster::RosterPlugin));
    #[cfg(feature = "gain")]
    plugins.push(Box::new(gain::GainPlugin));
    #[cfg(feature = "room")]
    plugins.push(Box::new(room::RoomPlugin));
    #[cfg(feature = "music")]
    plugins.push(Box::new(music::MusicPlugin));
    #[cfg(feature = "recording")]
    plugins.push(Box::new(recording::RecordingPlugin));
    #[cfg(feature = "playback")]
    plugins.push(Box::new(playback::PlaybackPlugin));
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_expected_plugins() {
        let plugins = all();
        let names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();
        assert!(names.contains(&"roster"));
        assert!(names.contains(&"gain"));
        assert!(names.contains(&"room"));
        assert!(names.contains(&"music"));
        assert!(names.contains(&"recording"));
        assert!(names.contains(&"playback"));
    }
}
