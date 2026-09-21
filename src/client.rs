// CLI 侧：daemon HTTP 客户端、自动拉起、自动解锁
use std::fs;
use std::io::IsTerminal;
use std::time::Duration;

use serde_json::{json, Value};

use crate::core::*;

pub struct ApiError {
    pub status: u16,
    pub obj: Value,
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = self
            .obj
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| self.obj.get("error").and_then(|e| e.as_str()))
            .unwrap_or("unknown");
        f.write_str(msg)
    }
}

pub fn api(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, ApiError> {
    let url = format!("http://127.0.0.1:{}{}", port(), path);
    let token = fs::read_to_string(client_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() >= 32);
    let sent = match method {
        "GET" => {
            let mut r = ureq::get(&url).timeout(timeout);
            if let Some(t) = &token {
                r = r.set("X-KeyX-Auth", t);
            }
            r.call()
        }
        "DELETE" => {
            let mut r = ureq::delete(&url).timeout(timeout);
            if let Some(t) = &token {
                r = r.set("X-KeyX-Auth", t);
            }
            r.call()
        }
        _ => {
            let mut r = ureq::post(&url).timeout(timeout);
            if let Some(t) = &token {
                r = r.set("X-KeyX-Auth", t);
            }
            let payload = body.cloned().unwrap_or_else(|| json!({}));
            r.send_string(&payload.to_string())
        }
    };
    let bad_resp = || ApiError { status: 0, obj: json!({ "error": "BAD_RESP", "message": "daemon 响应解析失败" }) };
    match sent {
        Ok(resp) => {
            let s = resp.into_string().map_err(|_| bad_resp())?;
            serde_json::from_str(&s).map_err(|_| bad_resp())
        }
        Err(ureq::Error::Status(code, resp)) => {
            let s = resp.into_string().unwrap_or_default();
            let obj = serde_json::from_str(&s).unwrap_or_else(|_| json!({}));
            Err(ApiError { status: code, obj })
        }
        Err(_) => Err(ApiError { status: 0, obj: json!({ "error": "CONN", "message": "无法连接 daemon" }) }),
    }
}

pub fn api_or_die(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Value {
    match api(method, path, body, timeout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

pub fn ensure_daemon() {
    ensure_home();
    match api("GET", "/status", None, Duration::from_secs(1)) {
        Ok(_) => return,
        Err(e) if e.status == 403 => {
            eprintln!(
                "端口上的 daemon 与本机密库不匹配（client token 校验失败）。\n可能上次异常退出留下了旧 daemon：运行 `keyx stop`，或用 KEYX_PORT 换端口。"
            );
            std::process::exit(1);
        }
        // 有 HTTP 响应但状态码异常：daemon 在（例如 404 探测路径），交由后续命令处理
        Err(e) if e.status != 0 => return,
        Err(_) => {}
    }
    // 拉起 daemon（日志进 daemon.log）
    let log = fs::OpenOptions::new().create(true).append(true).open(daemon_log()).expect("open daemon.log");
    let exe = std::env::current_exe().expect("current_exe");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let _ = std::process::Command::new(&exe)
            .arg("serve")
            .stdout(log.try_clone().expect("clone log"))
            .stderr(log)
            .process_group(0)
            .spawn();
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new(&exe).arg("serve").stdout(log.try_clone().unwrap()).stderr(log).spawn();
    }
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(100));
        match api("GET", "/status", None, Duration::from_secs(1)) {
            Ok(_) => return,
            Err(e) if e.status != 0 => return,
            Err(_) => {}
        }
    }
    eprintln!("daemon 启动失败，查看 ~/.keyx/daemon.log");
    std::process::exit(1);
}

pub fn read_password(prompt: &str) -> Option<String> {
    if std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt).ok()
    } else {
        std::env::var("KEYX_PASSWORD").ok().filter(|s| !s.is_empty())
    }
}

pub const NONINTERACTIVE_HINT: &str =
    "daemon 已锁定，且当前环境无法交互输入密码（例如由 AI agent 调用）。\n请人工在终端运行: keyx unlock";

pub fn ensure_unlocked() {
    let st = api_or_die("GET", "/status", None, Duration::from_secs(5));
    if st.get("unlocked").and_then(|v| v.as_bool()).unwrap_or(false) {
        return;
    }
    if !vault_file().exists() {
        // 默认钥匙串模式：用户不设置、不记忆任何密码；无钥匙串的环境回退主密码
        match api("POST", "/create", Some(&json!({ "mode": "keychain" })), Duration::from_secs(15)) {
            Ok(_) => {
                println!("密库已创建（钥匙串模式，跟随系统登录，无需记忆任何密码）。");
                return;
            }
            Err(e) => {
                if e.obj.get("error").and_then(|v| v.as_str()) != Some("NO_KEYRING") {
                    eprintln!("创建失败：{}", e);
                    std::process::exit(1);
                }
            }
        }
        let pw = match read_password("设置主密码（至少 8 位）: ") {
            Some(p) => p,
            None => {
                eprintln!("{}\n（首次使用请人工运行: keyx status，按提示创建密库）", NONINTERACTIVE_HINT);
                std::process::exit(1);
            }
        };
        if pw.chars().count() < 8 {
            eprintln!("主密码至少 8 位");
            std::process::exit(1);
        }
        let pw2 = read_password("确认主密码: ").unwrap_or_default();
        if pw != pw2 {
            eprintln!("两次输入不一致");
            std::process::exit(1);
        }
        api_or_die("POST", "/create", Some(&json!({ "password": pw })), Duration::from_secs(30));
        println!("密码库已创建（主密码模式，仅存本机，主密码丢失无法找回）。");
        return;
    }
    if st.get("mode").and_then(|v| v.as_str()) == Some("keychain") {
        if let Err(e) = api("POST", "/unlock", Some(&json!({})), Duration::from_secs(15)) {
            eprintln!("解锁失败：{}", e);
            std::process::exit(1);
        }
        return;
    }
    let pw = match read_password("主密码: ") {
        Some(p) => p,
        None => {
            eprintln!("{}", NONINTERACTIVE_HINT);
            std::process::exit(1);
        }
    };
    if let Err(e) = api("POST", "/unlock", Some(&json!({ "password": pw })), Duration::from_secs(30)) {
        eprintln!("解锁失败：{}", e);
        std::process::exit(1);
    }
}
