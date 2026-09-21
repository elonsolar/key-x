// peek：系统弹窗人工确认，明文只进剪贴板或弹窗本身，绝不写 stdout/stderr/文件
use std::io::Write;
use std::time::Duration;

pub const CLIPBOARD_CLEAR_SEC: u64 = 45;

pub fn peek_popup(name: &str, value: &str) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        macos_popup(name, value)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows/Linux 桌面弹窗待做（Windows 未真机验证，不假装支持）：直接复制，尽力而为
        if copy_clipboard(value) {
            schedule_clear(value.to_string());
            Some("clipboard")
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_popup(name: &str, value: &str) -> Option<&'static str> {
    // 无桌面环境（SSH）时 osascript 直接失败 → 返回 None 走 PEEK_NO_GUI
    let probe = std::process::Command::new("osascript").arg("-e").arg("return 1").output().ok()?;
    if !probe.status.success() {
        return None;
    }
    let msg = format!("「{}」的密码\n\n复制到剪贴板？（{} 秒后自动清空剪贴板）", name, CLIPBOARD_CLEAR_SEC);
    let script = format!(
        "display dialog {} with title \"keyx\" buttons {{\"取消\", \"显示\", \"复制\"}} default button \"复制\" with icon note",
        as_quote(&msg)
    );
    let out = std::process::Command::new("osascript").arg("-e").arg(&script).output().ok()?;
    if !out.status.success() {
        return Some("cancelled"); // 用户点了取消 / Esc
    }
    if String::from_utf8_lossy(&out.stdout).contains("显示") {
        let msg2 = format!("「{}」的密码（可选中后 ⌘C 复制）：", name);
        let script2 = format!(
            "display dialog {} default answer {} with title \"keyx\" buttons {{\"好\"}} default button \"好\" with icon note",
            as_quote(&msg2),
            as_quote(value)
        );
        let _ = std::process::Command::new("osascript").arg("-e").arg(&script2).output();
        return Some("show");
    }
    if copy_clipboard(value) {
        schedule_clear(value.to_string());
        Some("clipboard")
    } else {
        None
    }
}

fn as_quote(s: &str) -> String {
    let out = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n");
    format!("\"{}\"", out)
}

#[cfg(target_os = "macos")]
pub fn copy_clipboard(value: &str) -> bool {
    match std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(si) = child.stdin.as_mut() {
                let _ = si.write_all(value.as_bytes());
            }
            let _ = child.wait();
            true
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
pub fn copy_clipboard(value: &str) -> bool {
    match std::process::Command::new("clip").stdin(std::process::Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(si) = child.stdin.as_mut() {
                let _ = si.write_all(value.as_bytes());
            }
            let _ = child.wait();
            true
        }
        Err(_) => false,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn copy_clipboard(value: &str) -> bool {
    for cmd in [["wl-copy"], ["xclip", "-selection", "clipboard"]] {
        let mut c = std::process::Command::new(cmd[0]);
        c.args(&cmd[1..]).stdin(std::process::Stdio::piped());
        if let Ok(mut child) = c.spawn() {
            if let Some(si) = child.stdin.as_mut() {
                let _ = si.write_all(value.as_bytes());
            }
            let _ = child.wait();
            return true;
        }
    }
    false
}

/// 仅 macOS 严格对比，避免清掉用户之后复制的新内容；其他平台尽力直接清
fn clipboard_matches(value: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pbpaste")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout) == value)
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = value;
        true
    }
}

pub fn schedule_clear(value: String) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(CLIPBOARD_CLEAR_SEC));
        if clipboard_matches(&value) {
            copy_clipboard("");
        }
    });
}
