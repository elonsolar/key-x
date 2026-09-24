// peek：系统弹窗人工确认，明文只进剪贴板或弹窗本身，绝不写 stdout/stderr/文件
// 三平台行为对齐：弹窗三键（取消/显示/复制）。无 GUI 可用（SSH/无头服务器/缺弹窗组件）
// 一律返回 None 走 PEEK_NO_GUI 拒绝，绝不静默降级直接复制
use std::time::Duration;

pub const CLIPBOARD_CLEAR_SEC: u64 = 45;

pub fn peek_popup(name: &str, value: &str) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        macos_popup(name, value)
    }
    #[cfg(target_os = "windows")]
    {
        windows_popup(name, value)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_popup(name, value)
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

#[cfg(target_os = "macos")]
fn as_quote(s: &str) -> String {
    let out = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n");
    format!("\"{}\"", out)
}

#[cfg(target_os = "windows")]
fn windows_popup(name: &str, value: &str) -> Option<&'static str> {
    // 无桌面会话（SSH 等）里 ShowDialog 失败 → PowerShell 非零退出 → None 走 PEEK_NO_GUI
    let msg = format!("「{}」的密码\n\n复制到剪贴板？（{} 秒后自动清空剪贴板）", name, CLIPBOARD_CLEAR_SEC);
    let script = PS_CONFIRM.replace("@MSG@", &b64(&msg));
    match run_powershell(&script)?.as_str() {
        "copy" => {
            if copy_clipboard(value) {
                schedule_clear(value.to_string());
                Some("clipboard")
            } else {
                None
            }
        }
        "show" => {
            let msg2 = format!("「{}」的密码（可选中后 Ctrl+C 复制）：", name);
            let script2 = PS_SHOW.replace("@MSG@", &b64(&msg2)).replace("@VALUE@", &b64(value));
            let _ = run_powershell(&script2);
            Some("show")
        }
        _ => Some("cancelled"), // 点了取消 / Esc / 直接关窗
    }
}

// WinForms 确认窗体：取消/显示/复制 三键，Enter 默认复制，Esc 取消。
// 文案与密码走 base64 占位符注入，规避 PowerShell 引号/特殊字符转义与注入
#[cfg(target_os = "windows")]
const PS_CONFIRM: &str = r#"Add-Type -AssemblyName System.Windows.Forms
$msg = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('@MSG@'))
$f = New-Object System.Windows.Forms.Form
$f.Text = 'keyx'
$f.ClientSize = New-Object System.Drawing.Size(480, 190)
$f.StartPosition = 'CenterScreen'
$f.TopMost = $true
$f.FormBorderStyle = 'FixedDialog'
$f.MaximizeBox = $false
$f.MinimizeBox = $false
$l = New-Object System.Windows.Forms.Label
$l.Text = $msg
$l.Location = New-Object System.Drawing.Point(25, 20)
$l.Size = New-Object System.Drawing.Size(430, 100)
$f.Controls.Add($l)
$bc = New-Object System.Windows.Forms.Button
$bc.Text = '复制'
$bc.Size = New-Object System.Drawing.Size(100, 30)
$bc.Location = New-Object System.Drawing.Point(50, 135)
$bc.Add_Click({ $this.FindForm().Tag = 'copy'; $this.FindForm().Close() })
$f.Controls.Add($bc)
$bs = New-Object System.Windows.Forms.Button
$bs.Text = '显示'
$bs.Size = New-Object System.Drawing.Size(100, 30)
$bs.Location = New-Object System.Drawing.Point(190, 135)
$bs.Add_Click({ $this.FindForm().Tag = 'show'; $this.FindForm().Close() })
$f.Controls.Add($bs)
$bx = New-Object System.Windows.Forms.Button
$bx.Text = '取消'
$bx.Size = New-Object System.Drawing.Size(100, 30)
$bx.Location = New-Object System.Drawing.Point(330, 135)
$bx.Add_Click({ $this.FindForm().Tag = 'cancel'; $this.FindForm().Close() })
$f.Controls.Add($bx)
$f.AcceptButton = $bc
$f.CancelButton = $bx
[void]$f.ShowDialog()
$r = $f.Tag
$f.Dispose()
$r
"#;

// WinForms 显示窗体：密码放进 TextBox，可选中后 Ctrl+C
#[cfg(target_os = "windows")]
const PS_SHOW: &str = r#"Add-Type -AssemblyName System.Windows.Forms
$msg = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('@MSG@'))
$val = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('@VALUE@'))
$f = New-Object System.Windows.Forms.Form
$f.Text = 'keyx'
$f.ClientSize = New-Object System.Drawing.Size(520, 170)
$f.StartPosition = 'CenterScreen'
$f.TopMost = $true
$f.FormBorderStyle = 'FixedDialog'
$f.MaximizeBox = $false
$f.MinimizeBox = $false
$l = New-Object System.Windows.Forms.Label
$l.Text = $msg
$l.Location = New-Object System.Drawing.Point(25, 20)
$l.Size = New-Object System.Drawing.Size(470, 40)
$f.Controls.Add($l)
$t = New-Object System.Windows.Forms.TextBox
$t.Text = $val
$t.Location = New-Object System.Drawing.Point(25, 70)
$t.Size = New-Object System.Drawing.Size(470, 24)
$f.Controls.Add($t)
$b = New-Object System.Windows.Forms.Button
$b.Text = '好'
$b.Size = New-Object System.Drawing.Size(100, 30)
$b.Location = New-Object System.Drawing.Point(210, 115)
$b.Add_Click({ $this.FindForm().Close() })
$f.Controls.Add($b)
$f.AcceptButton = $b
$f.CancelButton = $b
[void]$f.ShowDialog()
$f.Dispose()
"#;

#[cfg(target_os = "windows")]
fn b64(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[cfg(target_os = "windows")]
fn run_powershell(script: &str) -> Option<String> {
    // -EncodedCommand 收 UTF-16LE base64 脚本，彻底绕开命令行转义
    let mut utf16 = Vec::with_capacity(script.len() * 2 + 2);
    for u in script.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let encoded = b64_bytes(&utf16);
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "windows")]
fn b64_bytes(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_popup(name: &str, value: &str) -> Option<&'static str> {
    // 无显示服务器（SSH/无头）→ 和 macOS over SSH 一样拒绝，走 PEEK_NO_GUI
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    // 优先 zenity（GNOME 系），其次 kdialog（KDE 系）；都没有 → 拒绝，不静默降级
    if has_cmd("zenity") {
        zenity_popup(name, value)
    } else if has_cmd("kdialog") {
        kdialog_popup(name, value)
    } else {
        None
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn has_cmd(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(cmd).is_file()))
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn zenity_popup(name: &str, value: &str) -> Option<&'static str> {
    let msg = format!("「{}」的密码\n\n复制到剪贴板？（{} 秒后自动清空剪贴板）", name, CLIPBOARD_CLEAR_SEC);
    // 点「显示」时 zenity 把按钮名打到 stdout；点「复制」exit 0 无输出；取消/Esc exit 1
    let out = std::process::Command::new("zenity")
        .args([
            "--question",
            "--title",
            "keyx",
            "--no-wrap",
            "--ok-label",
            "复制",
            "--cancel-label",
            "取消",
            "--extra-button",
            "显示",
            "--text",
            &msg,
        ])
        .output()
        .ok()?;
    if String::from_utf8_lossy(&out.stdout).trim() == "显示" {
        let msg2 = format!("「{}」的密码（可选中后 Ctrl+C 复制）：", name);
        let _ = std::process::Command::new("zenity")
            .args(["--entry", "--title", "keyx", "--text", &msg2, "--entry-text", value])
            .output();
        return Some("show");
    }
    if out.status.success() {
        if copy_clipboard(value) {
            schedule_clear(value.to_string());
            Some("clipboard")
        } else {
            None
        }
    } else {
        Some("cancelled")
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn kdialog_popup(name: &str, value: &str) -> Option<&'static str> {
    let msg = format!("「{}」的密码\n\n复制到剪贴板？（{} 秒后自动清空剪贴板）", name, CLIPBOARD_CLEAR_SEC);
    // --warningyesnocancel 退出码：0=是(复制) 1=否(显示) 2/Esc=取消
    let out = std::process::Command::new("kdialog")
        .args(["--title", "keyx", "--warningyesnocancel", &msg])
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => {
            if copy_clipboard(value) {
                schedule_clear(value.to_string());
                Some("clipboard")
            } else {
                None
            }
        }
        Some(1) => {
            let msg2 = format!("「{}」的密码（可选中后 Ctrl+C 复制）：", name);
            let _ = std::process::Command::new("kdialog")
                .args(["--title", "keyx", "--inputbox", &msg2, value])
                .output();
            Some("show")
        }
        _ => Some("cancelled"),
    }
}

#[cfg(target_os = "macos")]
pub fn copy_clipboard(value: &str) -> bool {
    use std::io::Write;
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
    // 走 Set-Clipboard 而非 clip.exe：后者按控制台代码页处理管道输入，非 ASCII 会乱码
    if value.is_empty() {
        // 清空剪贴板（schedule_clear 用）；Set-Clipboard 对空串行为不稳，用 Clipboard::Clear
        run_powershell("Add-Type -AssemblyName System.Windows.Forms\n[System.Windows.Forms.Clipboard]::Clear()").is_some()
    } else {
        let script =
            format!("Set-Clipboard -Value ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}')))", b64(value));
        run_powershell(&script).is_some()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn copy_clipboard(value: &str) -> bool {
    use std::io::Write;
    let cmds: [&[&str]; 2] = [&["wl-copy"], &["xclip", "-selection", "clipboard"]];
    for cmd in cmds {
        let mut c = std::process::Command::new(cmd[0]);
        c.args(&cmd[1..]).stdin(std::process::Stdio::piped());
        if let Ok(mut child) = c.spawn() {
            if let Some(si) = child.stdin.as_mut() {
                let _ = si.write_all(value.as_bytes());
            }
            // 连不上显示服务器时工具会非零退出（xclip/wl-copy 服务进程会先 fork，父进程退出码仍可信）
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    false
}
/// 只在剪贴板内容仍是本条密码时才清，避免清掉用户之后复制的新内容；读不到就宁可不清
fn clipboard_matches(value: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pbpaste")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout) == value)
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        run_powershell("[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Raw")
            .map(|s| s == value)
            .unwrap_or(false)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_read_clipboard().map(|s| s == value).unwrap_or(false)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_read_clipboard() -> Option<String> {
    let cmds: [&[&str]; 2] = [&["wl-paste"], &["xclip", "-o", "-selection", "clipboard"]];
    for cmd in cmds {
        let mut c = std::process::Command::new(cmd[0]);
        c.args(&cmd[1..]);
        if let Ok(out) = c.output() {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string());
            }
        }
    }
    None
}

pub fn schedule_clear(value: String) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(CLIPBOARD_CLEAR_SEC));
        if clipboard_matches(&value) {
            copy_clipboard("");
        }
    });
}
