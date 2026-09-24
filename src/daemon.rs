// daemon：127.0.0.1 HTTP 服务，client token 鉴权，内存持钥，闲置自动锁定
use std::collections::HashSet;
use std::fs;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use crate::core::*;
use crate::peek;

pub struct Unlocked {
    pub key: Vec<u8>,
    pub data: Value,
    pub kdf: String,
}

pub struct State {
    pub unlocked: Mutex<Option<Unlocked>>,
    pub last_used: Mutex<std::time::Instant>,
    pub lock_minutes: u64,
}

impl State {
    pub fn new() -> Self {
        State {
            unlocked: Mutex::new(None),
            last_used: Mutex::new(std::time::Instant::now()),
            lock_minutes: lock_minutes(),
        }
    }
    fn touch(&self) {
        *self.last_used.lock().unwrap() = std::time::Instant::now();
    }
    /// 自动锁检查 + 取出解锁态执行操作。锁内不做慢操作（peek 先取出值再弹窗）。
    pub fn with<T>(&self, f: impl FnOnce(&mut Unlocked) -> Result<T, VaultError>) -> Result<T, VaultError> {
        let mut g = self.unlocked.lock().unwrap();
        if g.is_some() && self.lock_minutes > 0 && self.last_used.lock().unwrap().elapsed() > Duration::from_secs(self.lock_minutes * 60) {
            *g = None;
            audit("LOCK", json!({ "reason": "autolock", "idle_min": self.lock_minutes }));
        }
        match g.as_mut() {
            None => Err(VaultError::new("LOCKED", "daemon 处于锁定状态，请先 keyx unlock")),
            Some(v) => {
                self.touch();
                f(v)
            }
        }
    }
    pub fn lock_manual(&self, reason: &str) -> bool {
        let mut g = self.unlocked.lock().unwrap();
        let had = g.is_some();
        *g = None;
        if had {
            audit("LOCK", json!({ "reason": reason }));
        }
        had
    }
}

pub fn port_alive(p: u16) -> bool {
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", p).parse().unwrap();
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

pub fn serve() -> ! {
    ensure_home();
    if pid_file().exists() {
        let pid = fs::read_to_string(pid_file()).ok().and_then(|s| s.trim().parse::<u32>().ok());
        if let Some(pid) = pid {
            if port_alive(port()) {
                eprintln!("daemon 已在运行 (pid {})，端口 {}", pid, port());
                std::process::exit(1);
            }
        }
        let _ = fs::remove_file(pid_file());
    }
    let token = to_hex(&random_bytes(32));
    fs::write(client_file(), format!("{}\n", token)).expect("write client.token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(client_file(), fs::Permissions::from_mode(0o600));
    }
    fs::write(pid_file(), format!("{}\n", std::process::id())).ok();
    let pf = pid_file();
    let _ = ctrlc::set_handler(move || {
        let _ = fs::remove_file(&pf);
        std::process::exit(0);
    });
    audit("START", json!({ "port": port(), "pid": std::process::id() }));
    println!(
        "Key-X daemon v{} 监听 127.0.0.1:{} (pid {})；闲置 {} 分钟自动清除密钥",
        VERSION,
        port(),
        std::process::id(),
        lock_minutes()
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let server = Arc::new(Server::http(("127.0.0.1", port())).expect("bind 127.0.0.1"));
    let state = Arc::new(State::new());
    for _ in 0..8 {
        let server = server.clone();
        let state = state.clone();
        std::thread::spawn(move || loop {
            match server.recv() {
                Ok(req) => handle(req, &state),
                Err(_) => break,
            }
        });
    }
    loop {
        std::thread::park();
    }
}

fn handle(mut req: tiny_http::Request, state: &State) {
    let mut auth = String::new();
    for h in req.headers() {
        if h.field.as_str().to_string().eq_ignore_ascii_case("x-keyx-auth") {
            auth = h.value.as_str().to_string();
        }
    }
    let method = req.method().to_string().to_uppercase();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let bodyv: Value = serde_json::from_str(body.trim()).unwrap_or_else(|_| json!({}));

    let expect = fs::read_to_string(client_file()).map(|s| s.trim().to_string()).unwrap_or_default();
    let (status, obj) = if expect.len() < 32 || auth != expect {
        (403, json!({ "error": "FORBIDDEN", "message": "client token 不匹配" }))
    } else {
        match route(state, &method, &path, &url, &bodyv) {
            Ok(pair) => pair,
            Err(ve) => (status_for(ve.code), json!({ "error": ve.code, "message": ve.message })),
        }
    };
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap();
    let resp = Response::from_string(obj.to_string()).with_status_code(status).with_header(header);
    let _ = req.respond(resp);
}

pub fn status_for(code: &'static str) -> u16 {
    match code {
        "WRONG_PASSWORD" => 401,
        "LOCKED" | "KEYCHAIN_MISSING" => 423,
        "KEYCHAIN_OCCUPIED" | "EXISTS" | "NAME_EXISTS" => 409,
        "NO_VAULT" | "NOT_FOUND" => 404,
        "VAULT_CORRUPT" => 422,
        _ => 400,
    }
}

fn status_obj(state: &State) -> Value {
    let g = state.unlocked.lock().unwrap();
    let unlocked = g.is_some();
    let entries = g.as_ref().map(|v| v.data["entries"].as_array().map(|a| a.len()).unwrap_or(0));
    let kdf = g
        .as_ref()
        .map(|v| v.kdf.clone())
        .or_else(|| load_store().ok().flatten().and_then(|s| s.get("kdf").and_then(|k| k.as_str()).map(String::from)));
    drop(g);
    let mode = match kdf.as_deref() {
        Some("KEYCHAIN") => Some("keychain"),
        Some(MODE_PASSWORD) => Some("password"),
        _ => None,
    };
    json!({ "unlocked": unlocked, "entries": entries, "mode": mode, "autolock_min": state.lock_minutes, "version": VERSION })
}

fn route(state: &State, method: &str, path: &str, url: &str, body: &Value) -> Result<(u16, Value), VaultError> {
    match (method, path) {
        ("GET", "/status") => Ok((200, status_obj(state))),
        ("GET", "/entries") => state.with(|v| {
            let out: Vec<Value> = v
                .data["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|e| {
                    json!({
                        "id": e["id"], "name": e["name"], "username": e["username"], "url": e["url"],
                        "has_password": e.get("password").and_then(|p| p.as_str()).map(|s| !s.is_empty()).unwrap_or(false),
                        "updatedAt": e["updatedAt"],
                    })
                })
                .collect();
            Ok((200, json!({ "entries": out })))
        }),
        ("GET", "/trust") => Ok((200, json!({ "trust": trust_load() }))),
        ("GET", "/audit") => {
            let n: usize = url
                .split("n=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(20);
            let lines: Vec<Value> = fs::read_to_string(audit_file())
                .map(|s| s.lines().map(|l| json!(l)).collect())
                .unwrap_or_default();
            let start = lines.len().saturating_sub(n);
            Ok((200, json!({ "lines": &lines[start..] })))
        }
        ("POST", "/create") => {
            let mut g = state.unlocked.lock().unwrap();
            if load_store()?.is_some() {
                return Err(VaultError::new("EXISTS", "密码库已存在"));
            }
            let mode = resolve_create_mode(body)?;
            let pw = body.get("password").and_then(|v| v.as_str()).map(String::from);
            let (key, data) = vault_create(pw.as_deref(), &mode)?;
            *g = Some(Unlocked { key, data, kdf: mode.clone() });
            state.touch();
            drop(g);
            audit("CREATE_VAULT", json!({ "mode": mode.clone() }));
            Ok((200, json!({ "ok": true, "mode": mode })))
        }
        ("POST", "/unlock") => {
            let mut g = state.unlocked.lock().unwrap();
            let pw = body.get("password").and_then(|v| v.as_str()).map(String::from);
            let (key, data, kdf) = vault_unlock(pw.as_deref())?;
            *g = Some(Unlocked { key, data, kdf: kdf.clone() });
            state.touch();
            drop(g);
            audit("UNLOCK", json!({ "ok": true, "mode": kdf }));
            Ok((200, json!({ "ok": true })))
        }
        ("POST", "/lock") => {
            state.lock_manual("manual");
            Ok((200, json!({ "ok": true })))
        }
        ("POST", "/entries") => entries_post(state, body),
        ("POST", "/resolve") => resolve(state, body),
        ("POST", "/trust") => {
            let project = body.get("project").and_then(|v| v.as_str()).unwrap_or("").trim_end_matches('/').to_string();
            let keys: Vec<Value> = body.get("keys").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if project.is_empty() || keys.is_empty() {
                return Err(VaultError::new("BAD_REQUEST", "project 与 keys 必填"));
            }
            let mut entries = trust_load();
            let mut merged: Vec<String> = entries
                .iter()
                .find(|e| e["project"].as_str() == Some(project.as_str()))
                .and_then(|e| e["keys"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|k| k.as_str().map(String::from))
                .collect();
            for k in keys.iter().filter_map(|k| k.as_str()) {
                if !merged.iter().any(|m| m == k) {
                    merged.push(k.to_string());
                }
            }
            merged.sort();
            let kv: Vec<Value> = merged.iter().map(|s| json!(s)).collect();
            if let Some(e) = entries.iter_mut().find(|e| e["project"].as_str() == Some(project.as_str())) {
                e["keys"] = json!(kv);
            } else {
                entries.push(json!({ "project": project, "keys": kv, "addedAt": now_iso() }));
            }
            trust_save(&entries);
            audit("TRUST", json!({ "project": project, "keys": keys }));
            Ok((200, json!({ "ok": true })))
        }
        ("POST", "/peek") => peek_route(state, body),
        ("DELETE", "/entries") => {
            let kid = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            state.with(move |v| {
                let arr = v
                    .data["entries"]
                    .as_array_mut()
                    .ok_or_else(|| VaultError::new("VAULT_CORRUPT", "密库内部结构异常"))?;
                let before = arr.len();
                arr.retain(|e| e["id"].as_str() != Some(kid.as_str()));
                if arr.len() == before {
                    return Err(VaultError::new("NOT_FOUND", format!("不存在 {}", kid)));
                }
                vault_save(&v.key, &v.data)?;
                audit("DELETE", json!({ "key": kid }));
                Ok((200, json!({ "ok": true })))
            })
        }
        ("DELETE", "/trust") => {
            let project = body.get("project").and_then(|v| v.as_str()).unwrap_or("").trim_end_matches('/').to_string();
            let all = trust_load();
            let entries: Vec<Value> = all
                .iter()
                .filter(|e| e["project"].as_str() != Some(project.as_str()))
                .cloned()
                .collect();
            trust_save(&entries);
            audit("UNTRUST", json!({ "project": project }));
            Ok((200, json!({ "ok": true })))
        }
        _ => Err(VaultError::new("NOT_FOUND", "no route")),
    }
}

fn entries_post(state: &State, body: &Value) -> Result<(u16, Value), VaultError> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let kid = body.get("id").and_then(|v| v.as_str()).map(String::from);
    state.with(move |v| {
        if let Some(kid) = kid {
            let entries = v
                .data["entries"]
                .as_array_mut()
                .ok_or_else(|| VaultError::new("VAULT_CORRUPT", "密库内部结构异常"))?;
            if let Some(new_name) = body.get("name").and_then(|n| n.as_str()) {
                if entries
                    .iter()
                    .any(|e| e["id"].as_str() != Some(kid.as_str()) && e["name"].as_str() == Some(new_name))
                {
                    return Err(VaultError::new(
                        "NAME_EXISTS",
                        format!("名称「{new_name}」已被其他条目使用"),
                    ));
                }
            }
            let entry = entries
                .iter_mut()
                .find(|e| e["id"].as_str() == Some(kid.as_str()))
                .ok_or_else(|| VaultError::new("NOT_FOUND", format!("不存在 {}", kid)))?;
            let mut changed: Vec<Value> = vec![];
            for f in ["name", "username", "url", "notes", "password"] {
                if let Some(val) = body.get(f) {
                    entry[f] = val.clone();
                    changed.push(json!(f));
                }
            }
            entry["updatedAt"] = json!(now_iso());
            vault_save(&v.key, &v.data)?;
            let ev = if changed.contains(&json!("password")) { "ROTATE" } else { "UPDATE" };
            audit(ev, json!({ "key": kid, "fields": changed }));
            Ok((200, json!({ "id": kid })))
        } else {
            if name.is_empty() {
                return Err(VaultError::new("BAD_REQUEST", "name 必填"));
            }
            let entries = v
                .data["entries"]
                .as_array_mut()
                .ok_or_else(|| VaultError::new("VAULT_CORRUPT", "密库内部结构异常"))?;
            // 名称唯一：同名拒绝并指路（peek/rotate 按名操作才无歧义）
            if let Some(e) = entries.iter().find(|e| e["name"].as_str() == Some(name.as_str())) {
                let kid = e["id"].as_str().unwrap_or("");
                return Err(VaultError::new(
                    "NAME_EXISTS",
                    format!("名称「{name}」已存在（{kid}）。要换密码用 keyx rotate {kid}；要存不同的密码请换个名字。"),
                ));
            }
            let existing: HashSet<String> =
                entries.iter().filter_map(|e| e["id"].as_str().map(String::from)).collect();
            let new_kid = new_key_id(&existing);
            entries.push(json!({
                "id": new_kid, "name": name,
                "username": body.get("username").cloned().unwrap_or(json!("")),
                "url": body.get("url").cloned().unwrap_or(json!("")),
                "notes": body.get("notes").cloned().unwrap_or(json!("")),
                "password": body.get("password").cloned().unwrap_or(json!("")),
                "createdAt": now_iso(), "updatedAt": now_iso(),
            }));
            vault_save(&v.key, &v.data)?;
            audit("ADD", json!({ "key": new_kid, "name": name }));
            Ok((200, json!({ "id": new_kid })))
        }
    })
}

fn resolve(state: &State, body: &Value) -> Result<(u16, Value), VaultError> {
    let project = body.get("project").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let want: Vec<String> = body
        .get("keys")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    state.with(move |v| {
        let entries = v.data["entries"].as_array().cloned().unwrap_or_default();
        let find = |k: &str| entries.iter().find(|e| e["id"].as_str() == Some(k));
        let missing: Vec<String> = want.iter().filter(|k| find(k).is_none()).cloned().collect();
        if !missing.is_empty() {
            audit("DENY", json!({ "reason": "unknown_key", "project": project, "keys": missing }));
            return Err(VaultError::new("NOT_FOUND", format!("未知 key-id: {}", missing.join(", "))));
        }
        let need: Vec<String> = want.iter().filter(|k| !trust_allowed(&project, k)).cloned().collect();
        if !need.is_empty() {
            audit("DENY", json!({ "reason": "not_trusted", "project": project, "keys": need }));
            return Ok((403, json!({ "error": "NEED_TRUST", "project": project, "need": need })));
        }
        let mut out = serde_json::Map::new();
        for k in &want {
            if let Some(e) = find(k) {
                out.insert(k.clone(), e.get("password").cloned().unwrap_or(json!("")));
            }
        }
        audit("RESOLVE", json!({ "project": project, "keys": want }));
        Ok((200, json!({ "passwords": out })))
    })
}

fn peek_route(state: &State, body: &Value) -> Result<(u16, Value), VaultError> {
    let kid = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // headless 终端确认：client 已在真终端（TTY）拿到人工按键才带此标志，明文只进剪贴板不进 stdout
    let confirmed = body.get("confirm").and_then(|v| v.as_str()) == Some("copy");
    let (name, password) = state.with(|v| {
        let e = v
            .data["entries"]
            .as_array()
            .and_then(|a| a.iter().find(|e| e["id"].as_str() == Some(kid.as_str())))
            .cloned()
            .ok_or_else(|| VaultError::new("NOT_FOUND", format!("不存在 {}", kid)))?;
        Ok((
            e["name"].as_str().unwrap_or("").to_string(),
            e["password"].as_str().unwrap_or("").to_string(),
        ))
    })?;
    if std::env::var("KEYX_PEEK_TEST").is_ok() {
        audit("PEEK", json!({ "key": kid, "method": "test" }));
        return Ok((200, json!({ "ok": true, "method": "test" })));
    }
    let name = if name.is_empty() { kid.clone() } else { name };
    let method = if confirmed {
        if peek::copy_clipboard(&password) {
            peek::schedule_clear(password.clone());
            Some("clipboard")
        } else {
            None
        }
    } else {
        peek::peek_popup(&name, &password)
    };
    match method {
        None => {
            audit("PEEK", json!({ "key": kid, "method": "no_gui" }));
            Err(VaultError::new(
                "PEEK_NO_GUI",
                "此操作需要人工确认：无桌面环境（SSH/无头服务器）或缺少弹窗组件（Linux 需 zenity/kdialog）。请在人工终端重跑 keyx peek，按提示回车确认（仅复制模式）",
            ))
        }
        Some("cancelled") => {
            audit("PEEK", json!({ "key": kid, "method": "cancelled" }));
            Ok((200, json!({ "ok": false, "method": "cancelled" })))
        }
        Some(m) => {
            // 终端确认和弹窗确认在审计里区分开
            audit("PEEK", json!({ "key": kid, "method": if confirmed { "tty_confirm" } else { m } }));
            Ok((200, json!({ "ok": true, "method": m })))
        }
    }
}

