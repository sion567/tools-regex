//! 轻量应用日志 —— 零第三方依赖，双形态共享一套 API：
//! - 原生：追加写 exe 旁 `regex.log`（写不进则退回 %TEMP%），并镜像到 stderr；
//! - wasm：转发 `web_sys::console`（文件系统不可用，自动只剩 console）。
//!
//! 动机（闯关教学页"点了就崩"）：eframe 的 panic 只打在用户看不见的控制台上，
//! 窗口一关现场全丢。所以自带 panic 钩子，把 panic 消息 + 位置 + backtrace 落盘。
//! 不引入 log/env_logger——浏览器形态还得再拖 console_log，而 panic 落盘这块
//! 本来就要手写（依赖纪律见 scripts/lint_deps.sh）。
//!
//! 约定：只在**事件**处打点（选关/检查/切 tab/启动），不在每帧打——egui 是
//! immediate mode，show() 每秒跑 60 次，按帧打点会把日志刷爆。

#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, OnceLock};

#[cfg(not(target_arch = "wasm32"))]
const FILE_NAME: &str = "regex.log";

/// None = 文件打不开（只剩 stderr）。包一层 Option 让"试过但失败"也只发生一次。
#[cfg(not(target_arch = "wasm32"))]
static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

/// 启动时调用一次：建日志文件 + 装 panic 钩子 + 写启动横幅。
pub fn init(form: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let opened = open_file();
        let path = opened
            .as_ref()
            .map(|(_, p)| p.display().to_string())
            .unwrap_or_else(|| "<不可写，仅 stderr>".into());
        let _ = SINK.set(opened.map(|(f, _)| Mutex::new(f)));
        install_panic_hook();
        info(&format!(
            "启动 form={form} v={} 日志文件={path}",
            env!("CARGO_PKG_VERSION")
        ));
    }
    #[cfg(target_arch = "wasm32")]
    {
        install_panic_hook();
        info(&format!("启动 form={form} v={}", env!("CARGO_PKG_VERSION")));
    }
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

/// UI 用（历史记录时间戳）：当前 UTC 时间的 HH:MM:SS
pub fn now_hms() -> String {
    now_utc()[11..19].to_string()
}

/// panic 钩子：先落盘再走默认钩子（控制台红字照常可见）。
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<非字符串 panic 载荷>".into()
        };
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("<unnamed>");
        write("PANIC", &format!("崩溃于 {loc}（线程 {thread}）：{payload}"));
        #[cfg(not(target_arch = "wasm32"))]
        {
            // force_capture 不依赖 RUST_BACKTRACE 环境变量，release 也有符号
            let bt = std::backtrace::Backtrace::force_capture();
            write("PANIC", &format!("backtrace:\n{bt}"));
        }
        default_hook(info);
    }));
}

/// 原生：exe 旁优先（cargo run 即 target/debug/ 旁边），失败退 %TEMP%。
#[cfg(not(target_arch = "wasm32"))]
fn open_file() -> Option<(std::fs::File, std::path::PathBuf)> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for dir in [exe_dir, std::env::temp_dir()] {
        let path = dir.join(FILE_NAME);
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            return Some((f, path));
        }
    }
    None
}

fn write(tag: &str, msg: &str) {
    let line = format!("{} [{tag}] {msg}", now_utc());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{line}");
    #[cfg(target_arch = "wasm32")]
    web_sys::console::log_1(&line.into());
    // 锁内只有 writeln/flush（错误一律忽略，绝不 panic）——避免钩子重入死锁
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(Some(m)) = SINK.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

/// UTC 时间戳（std 无本地时区，排查对序足够）。日期用 Howard Hinnant 民历算法。
fn now_utc() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (secs, millis) = (d.as_secs(), d.subsec_millis());
    let (h, m, s) = (secs % 86400 / 3600, secs % 3600 / 60, secs % 60);
    let z = secs / 86400 + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 };
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}.{millis:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_well_formed() {
        let t = now_utc();
        // 2026-11-19 07:30:00.123
        assert_eq!(t.len(), 23);
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], " ");
        assert_eq!(&t[19..20], ".");
    }

    #[test]
    fn known_epoch_matches() {
        // 2026-01-01 00:00:00 UTC = 1767225600
        let secs = 1767225600u64;
        let (h, m, s) = (0, 0, 0);
        let z = secs / 86400 + 719_468;
        let era = z / 146_097;
        let doe = z % 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 };
        assert_eq!((year, month, day, h, m, s), (2026, 1, 1, 0, 0, 0));
    }

    /// 端到端验证"崩溃会留痕"：init → panic → 日志文件里必须有 PANIC 段。
    #[test]
    fn panic_hook_writes_to_file() {
        init("test");
        let path = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join(FILE_NAME);
        let before = std::fs::read_to_string(&path).unwrap_or_default().len();
        assert!(std::panic::catch_unwind(|| panic!("钩子自验证-marker")).is_err());
        let after = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(after.len() > before, "panic 后日志没有增长：{path:?}");
        assert!(after.contains("钩子自验证-marker"), "PANIC 消息未落盘：{path:?}");
        assert!(after.contains("[PANIC]"), "缺 [PANIC] 标记：{path:?}");
    }
}
