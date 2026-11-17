//! Flat `config.toml` next to the executable.
//!
//! The file is a handful of `key = integer` lines. A small parser keeps the
//! binary free of serde/toml.

use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

const DEFAULT_TEXT: &str = "\
# 久坐护眼提醒。修改后重新启动程序生效。
# 工作段时长（分钟）
work_minutes = 20
# 微休息时长（秒）
micro_break_seconds = 20
# 长休息时长（分钟）
long_break_minutes = 5
# 每隔多少个工作段安排一次长休息
long_break_every = 3
# 空闲判定。锁屏或睡眠超过此时长后，重置为新的工作段（分钟）
idle_minutes = 5
# 全屏或会议时，把这一次提醒推迟多久（分钟）
defer_minutes = 5
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub work: Duration,
    pub micro_break: Duration,
    pub long_break: Duration,
    pub long_break_every: u32,
    pub idle: Duration,
    pub defer: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work: Duration::from_secs(20 * 60),
            micro_break: Duration::from_secs(20),
            long_break: Duration::from_secs(5 * 60),
            long_break_every: 3,
            idle: Duration::from_secs(5 * 60),
            defer: Duration::from_secs(5 * 60),
        }
    }
}

pub fn load() -> Config {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => {
            let _ = std::fs::write(&path, DEFAULT_TEXT);
            Config::default()
        }
    }
}

pub fn config_path() -> PathBuf {
    exe_dir().join("config.toml")
}

fn exe_dir() -> PathBuf {
    let mut buffer = vec![0u16; 32_768];
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) };
    if length == 0 {
        return PathBuf::from(".");
    }
    let text = String::from_utf16_lossy(&buffer[..length as usize]);
    Path::new(&text)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn parse(text: &str) -> Config {
    let mut config = Config::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Ok(number) = value.trim().parse::<u64>() else {
            continue;
        };
        match key.trim() {
            "work_minutes" => {
                config.work = Duration::from_secs(clamp(number, 1, 12 * 60) * 60);
            }
            "micro_break_seconds" => {
                config.micro_break = Duration::from_secs(clamp(number, 5, 60 * 60));
            }
            "long_break_minutes" => {
                config.long_break = Duration::from_secs(clamp(number, 1, 180) * 60);
            }
            "long_break_every" => {
                config.long_break_every = clamp(number, 1, 100) as u32;
            }
            "idle_minutes" => {
                config.idle = Duration::from_secs(clamp(number, 1, 240) * 60);
            }
            "defer_minutes" => {
                config.defer = Duration::from_secs(clamp(number, 1, 120) * 60);
            }
            _ => {}
        }
    }
    config
}

fn clamp(value: u64, min: u64, max: u64) -> u64 {
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_keep_defaults_and_comments_are_ignored() {
        let config = parse("# comment\nwork_minutes = 25\n\nunknown = 9\n");
        assert_eq!(config.work, Duration::from_secs(25 * 60));
        assert_eq!(config.micro_break, Config::default().micro_break);
        assert_eq!(config.long_break_every, 3);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let config = parse(
            "work_minutes = 0\nmicro_break_seconds = 1\nlong_break_every = 0\nidle_minutes = 9999\n",
        );
        assert_eq!(config.work, Duration::from_secs(60));
        assert_eq!(config.micro_break, Duration::from_secs(5));
        assert_eq!(config.long_break_every, 1);
        assert_eq!(config.idle, Duration::from_secs(240 * 60));
    }
}
