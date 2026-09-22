// CLI 命令实现。输出文本是对外契约——测试按文本断言，改动需同步测试
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::client::{api, api_or_die, ensure_daemon, ensure_unlocked, read_password, NONINTERACTIVE_HINT};
use crate::core::*;
use crate::peek::CLIPBOARD_CLEAR_SEC;

const T5: Duration = Duration::from_secs(5);
const T30: Duration = Duration::from_secs(30);

pub fn cmd_set(name: &str, username: Option<&str>, url: Option<&str>, notes: Option<&str>) {
    ensure_daemon();
    ensure_unlocked();
    let password = std::env::var("KEYX_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| read_password(&format!("「{}」的密码: ", name)))
        .unwrap_or_default();
    let r = api_or_die(
        "POST",
        "/entries",
        Some(&json!({
            "name": name,
            "username": username.unwrap_or(""),
            "url": url.unwrap_or(""),
            "notes": notes.unwrap_or(""),
            "password": password,
        })),
        T5,
    );
    let id = r["id"].as_str().unwrap_or("").to_string();
    println!("已保存。key-id: {}   （配置里写 {} 引用 {} 即可）", id, name, id);
}

pub fn cmd_list(filter: Option<&str>) {
    ensure_daemon();
    ensure_unlocked();
    let mut entries = api_or_die("GET", "/entries", None, T5)["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // 按名称/用户名/备注子串过滤（不分大小写）：一个项目多条密码时按前缀归组查看
    if let Some(f) = filter {
        let f = f.to_lowercase();
        entries.retain(|e| {
            ["name", "username", "notes"].iter().any(|k| {
                e.get(k)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase().contains(&f))
                    .unwrap_or(false)
            })
        });
    }
    if entries.is_empty() {
        println!("（空）");
        return;
    }
    let mut sorted = entries;
    sorted.sort_by_key(|e| e["id"].as_str().unwrap_or("").to_string());
    let w = sorted
        .iter()
        .map(|e| e["name"].as_str().unwrap_or("").chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    println!("{:<14}{:<w$}{}", "key-id", "名称", "用户名", w = w);
    for e in sorted {
        println!(
            "{:<14}{:<w$}{}",
            e["id"].as_str().unwrap_or(""),
            e["name"].as_str().unwrap_or(""),
            e["username"].as_str().unwrap_or(""),
            w = w
        );
    }
}

pub fn cmd_rm(key_id: &str) {
    ensure_daemon();
    ensure_unlocked();
    api_or_die("DELETE", "/entries", Some(&json!({ "id": key_id })), T5);
    println!("已删除 {}", key_id);
}

pub fn cmd_peek(key: &str) {
    ensure_daemon();
    ensure_unlocked();
    let sel = key.trim();
    let entries = api_or_die("GET", "/entries", None, T5)["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let matches: Vec<&Value> = entries
        .iter()
        .filter(|e| e["id"].as_str() == Some(sel) || e["name"].as_str() == Some(sel))
        .collect();
    if matches.is_empty() {
        eprintln!("没有找到 {}。用 keyx list 查看编号与名称。", sel);
        std::process::exit(1);
    }
    if matches.len() > 1 {
        eprintln!("名称「{}」有 {} 条，请用编号指定：", sel, matches.len());
        for e in &matches {
            eprintln!("  {}  {}", e["id"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""));
        }
        std::process::exit(1);
    }
    let id = matches[0]["id"].as_str().unwrap_or("").to_string();
    let name = matches[0]["name"].as_str().unwrap_or("").to_string();
    // 弹窗等人工点击，超时给足 600s + 余量
    let r = match api("POST", "/peek", Some(&json!({ "id": id })), Duration::from_secs(660)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let name = if name.is_empty() { id } else { name };
    if !r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        println!("已取消。");
        return;
    }
    match r.get("method").and_then(|v| v.as_str()) {
        Some("clipboard") => println!("已把「{}」复制到剪贴板，{} 秒后自动清空。", name, CLIPBOARD_CLEAR_SEC),
        Some("show") => println!("已在弹窗中显示「{}」。", name),
        _ => println!("已确认。"),
    }
}

pub fn cmd_rotate(key_id: &str, username: Option<&str>) {
    ensure_daemon();
    ensure_unlocked();
    let mut pw = read_password(&format!("「{}」的新密码: ", key_id)).unwrap_or_default();
    if pw.is_empty() {
        pw = std::env::var("KEYX_SECRET").unwrap_or_default();
    }
    if pw.is_empty() {
        eprintln!("未提供新密码");
        std::process::exit(1);
    }
    let mut body = json!({ "id": key_id, "password": pw });
    if let Some(u) = username {
        body["username"] = json!(u);
    }
    if let Err(e) = api("POST", "/entries", Some(&body), T5) {
        if e.status == 404 {
            eprintln!("不存在 {}（keyx list 查看）", key_id);
            std::process::exit(1);
        }
        eprintln!("{}", e);
        std::process::exit(1);
    }
    println!("已轮换 {}：引用不变，配置无需改动。请记得在源头（数据库/云控制台）撤销旧值。", key_id);
}

pub fn is_kx_ref(s: &str) -> bool {
    s.len() > 3 && s.starts_with("kx_") && s.chars().skip(3).all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 解析链（先到先得）：显式 --env > .keyx.toml [env] > .env 中 kx_* 引用
pub fn collect_env_references(args_env: &[String], dir: &Path) -> Vec<(String, String)> {
    let mut mapping: Vec<(String, String)> = vec![];
    fn setdefault(m: &mut Vec<(String, String)>, k: String, v: String) {
        if !m.iter().any(|(a, _)| *a == k) {
            m.push((k, v));
        }
    }
    for kv in args_env {
        let (var, val) = kv.split_once('=').unwrap_or(("", ""));
        if var.trim().is_empty() || val.trim().is_empty() {
            eprintln!("--env 格式应为 变量名=key编号，收到: {}", kv);
            std::process::exit(1);
        }
        setdefault(&mut mapping, var.trim().to_string(), val.trim().to_string());
    }
    let toml_path = dir.join(".keyx.toml");
    if toml_path.exists() {
        match fs::read_to_string(&toml_path).ok().and_then(|s| toml::from_str::<toml::Value>(&s).ok()) {
            Some(tv) => {
                if let Some(envt) = tv.get("env").and_then(|v| v.as_table()) {
                    for (var, val) in envt {
                        if let Some(s) = val.as_str() {
                            if !s.trim().is_empty() {
                                setdefault(&mut mapping, var.clone(), s.trim().to_string());
                            }
                        }
                    }
                }
            }
            None => eprintln!("警告：.keyx.toml 解析失败，已忽略"),
        }
    }
    let env_path = dir.join(".env");
    if env_path.exists() {
        if let Ok(content) = fs::read_to_string(&env_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((var, val)) = line.split_once('=') else { continue };
                let var = var.trim();
                let val = val.trim().trim_matches('"').trim_matches('\'');
                if is_kx_ref(val) {
                    setdefault(&mut mapping, var.to_string(), val.to_string());
                }
            }
        }
    }
    mapping
}

#[cfg(unix)]
fn exec_cmd(cmd: &[String], env: HashMap<String, String>) -> ! {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&cmd[0]).args(&cmd[1..]).envs(env).exec();
    eprintln!("启动失败: {}", err);
    std::process::exit(127);
}
#[cfg(windows)]
fn exec_cmd(cmd: &[String], env: HashMap<String, String>) -> ! {
    match std::process::Command::new(&cmd[0]).args(&cmd[1..]).envs(env).spawn().and_then(|mut c| c.wait()) {
        Ok(st) => std::process::exit(st.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("启动失败: {}", e);
            std::process::exit(127);
        }
    }
}

pub fn cmd_run(envs: &[String], cmd: &[String]) {
    if cmd.is_empty() {
        eprintln!("用法: keyx run [--env 变量=编号]... -- <命令> [参数...]");
        std::process::exit(2);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let project = fs::canonicalize(&cwd)
        .unwrap_or_else(|_| cwd.clone())
        .to_string_lossy()
        .to_string();
    let mapping = collect_env_references(envs, &cwd);
    if mapping.is_empty() {
        // 无引用：纯透传，keyx 相当于不存在
        let env: HashMap<String, String> = std::env::vars().collect();
        exec_cmd(cmd, env);
    }
    ensure_daemon();
    ensure_unlocked();
    let mut keys: Vec<String> = mapping.iter().map(|(_, k)| k.clone()).collect();
    keys.sort();
    keys.dedup();
    let resolve = || api("POST", "/resolve", Some(&json!({ "project": project, "keys": keys })), T5);
    let r: Value = match resolve() {
        Ok(r) => r,
        Err(e) => {
            if e.status == 403 && e.obj.get("error").and_then(|v| v.as_str()) == Some("NEED_TRUST") {
                let need: Vec<String> = e
                    .obj
                    .get("need")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let entries = api_or_die("GET", "/entries", None, T5)["entries"].clone();
                // 询问一律写 stderr：stdout 只留给被启动命令的输出
                eprintln!("项目 {} 首次请求访问：", project);
                for k in &need {
                    let ent = entries
                        .as_array()
                        .and_then(|a| a.iter().find(|e| e["id"].as_str() == Some(k.as_str())).cloned())
                        .unwrap_or_else(|| json!({}));
                    eprintln!(
                        "  {}  {} {}",
                        k,
                        ent["name"].as_str().unwrap_or(""),
                        ent["username"].as_str().unwrap_or("")
                    );
                }
                eprint!("允许并记住本项目？[y/N] ");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                let ok = std::io::stdin()
                    .read_line(&mut line)
                    .map(|_| line.trim().eq_ignore_ascii_case("y"))
                    .unwrap_or(false);
                if !ok {
                    eprintln!("已取消");
                    std::process::exit(1);
                }
                api_or_die("POST", "/trust", Some(&json!({ "project": project, "keys": need })), T5);
                match resolve() {
                    Ok(v) => v,
                    Err(e2) => {
                        eprintln!("{}", e2);
                        std::process::exit(1);
                    }
                }
            } else if e.status == 423 {
                eprintln!("daemon 已锁定，尝试重新解锁…");
                ensure_unlocked();
                match resolve() {
                    Ok(v) => v,
                    Err(e2) => {
                        eprintln!("{}", e2);
                        std::process::exit(1);
                    }
                }
            } else {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    };
    let mut env: HashMap<String, String> = std::env::vars().collect();
    for (var, kid) in &mapping {
        match r["passwords"].get(kid).and_then(|p| p.as_str()) {
            Some(pw) => env.insert(var.clone(), pw.to_string()),
            None => {
                eprintln!("daemon 未返回 {} 的值", kid);
                std::process::exit(1);
            }
        };
    }
    exec_cmd(cmd, env);
}

pub fn cmd_stop() {
    if !pid_file().exists() {
        println!("daemon 未在运行");
        return;
    }
    let pid: u32 = match fs::read_to_string(pid_file()).ok().and_then(|s| s.trim().parse().ok()) {
        Some(p) => p,
        None => {
            let _ = fs::remove_file(pid_file());
            println!("清理了残留 pid 文件");
            return;
        }
    };
    #[cfg(unix)]
    {
        let ok = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = fs::remove_file(pid_file());
            println!("daemon 未在运行（清理了残留 pid 文件）");
            return;
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status();
    }
    for _ in 0..30 {
        if !crate::daemon::port_alive(port()) {
            println!("已停止 (pid {})", pid);
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("已发送停止信号 (pid {})，但端口仍在响应", pid);
}

pub fn cmd_trust(action: &str, keys: &[String], project: Option<&str>) {
    ensure_daemon();
    match action {
        "list" => {
            let entries = api_or_die("GET", "/trust", None, T5)["trust"].clone();
            let arr = entries.as_array().cloned().unwrap_or_default();
            if arr.is_empty() {
                println!("（空）");
            }
            for e in arr {
                println!("{}", e["project"].as_str().unwrap_or(""));
                let keys: Vec<&str> = e["keys"].as_array().map(|a| a.iter().filter_map(|k| k.as_str()).collect()).unwrap_or_default();
                println!("    {}", keys.join(" "));
            }
        }
        "add" => {
            if keys.is_empty() {
                eprintln!("需要至少一个 key-id");
                std::process::exit(1);
            }
            let p = project
                .map(|p| fs::canonicalize(p).unwrap_or_else(|_| Path::new(p).to_path_buf()))
                .unwrap_or_else(|| fs::canonicalize(".").unwrap_or_default())
                .to_string_lossy()
                .to_string();
            let kv: Vec<Value> = keys.iter().map(|s| json!(s)).collect();
            api_or_die("POST", "/trust", Some(&json!({ "project": p, "keys": kv })), T5);
            println!("已授权 {} 访问 {}", p, keys.join(" "));
        }
        "rm" => {
            let p = match project {
                Some(p) => fs::canonicalize(p).unwrap_or_else(|_| Path::new(p).to_path_buf()).to_string_lossy().to_string(),
                None => {
                    eprintln!("需要 --project 或在项目目录里执行");
                    std::process::exit(1);
                }
            };
            api_or_die("DELETE", "/trust", Some(&json!({ "project": p })), T5);
            println!("已撤销 {} 的授权", p);
        }
        _ => {
            eprintln!("trust 子命令：list | add <key-id>... [--project 目录] | rm [--project 目录]");
            std::process::exit(2);
        }
    }
}

pub fn cmd_audit(n: usize) {
    ensure_daemon();
    let lines = api_or_die("GET", &format!("/audit?n={}", n), None, T5)["lines"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for line in lines {
        match line.as_str() {
            Some(s) => match serde_json::from_str::<Value>(s) {
                Ok(e) => {
                    let extra = e
                        .as_object()
                        .map(|o| {
                            o.iter()
                                .filter(|(k, _)| *k != "ts" && *k != "event")
                                .map(|(k, v)| match v {
                                    Value::String(s) => format!("{}={}", k, s),
                                    other => format!("{}={}", k, other),
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    let ts = e["ts"].as_str().unwrap_or("");
                    let ev = e["event"].as_str().unwrap_or("");
                    println!("{}  {:<8} {}", &ts[..ts.len().min(19)], ev, extra);
                }
                Err(_) => println!("{}", s),
            },
            None => continue,
        }
    }
}

pub fn cmd_status() {
    ensure_daemon();
    let st = api_or_die("GET", "/status", None, T5);
    let state = if st["unlocked"].as_bool().unwrap_or(false) { "已解锁" } else { "已锁定" };
    let mode = match st["mode"].as_str() {
        Some("keychain") => "钥匙串",
        Some("password") => "主密码",
        _ => "无密库",
    };
    let mut s = format!("daemon: 运行中 (端口 {}, 自动锁定 {} 分钟) · 密库: {}", port(), st["autolock_min"], mode);
    if st["mode"].is_string() {
        s.push_str(&format!(" · {}", state));
    }
    if !st["entries"].is_null() {
        s.push_str(&format!(" · {} 条", st["entries"]));
    }
    println!("{}", s);
}

pub fn cmd_lock() {
    ensure_daemon();
    api_or_die("POST", "/lock", None, T5);
    println!("已锁定");
}

pub fn cmd_unlock() {
    ensure_daemon();
    ensure_unlocked();
    println!("已解锁");
}

pub fn cmd_export(out: Option<&str>, include_key: bool) {
    if !vault_file().exists() {
        eprintln!("没有可导出的密库");
        std::process::exit(1);
    }
    let mut content = fs::read_to_string(vault_file()).unwrap_or_default();
    let mut store: Value = serde_json::from_str(&content).unwrap_or(json!({}));
    if include_key {
        if store["kdf"].as_str() == Some(MODE_KEYCHAIN) {
            match keychain_get() {
                Some(hx) => {
                    store["key"] = json!(hx);
                    content = serde_json::to_string_pretty(&store).unwrap_or(content);
                    eprintln!("警告：导出文件包含主密钥，等于明文 —— 务必保密，导入后尽快删除。");
                }
                None => eprintln!("钥匙串中没有主密钥，导出不含密钥"),
            }
        } else {
            eprintln!("主密码模式的密库无需 include-key（备份本身就是密文）");
        }
    }
    audit("EXPORT", json!({ "include_key": include_key }));
    match out {
        Some(o) => {
            let _ = fs::write(o, content);
            println!("已导出（密文）→ {}", o);
        }
        None => print!("{}", content),
    }
}

pub fn cmd_import(file: &str, force: bool) {
    let raw = match fs::read_to_string(file) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("读取失败：{}", e);
            std::process::exit(1);
        }
    };
    let mut store: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("不是 Key-X 备份文件：{}", e);
            std::process::exit(1);
        }
    };
    if store["app"].as_str() != Some("key-x") {
        eprintln!("不是 Key-X 备份文件");
        std::process::exit(1);
    }
    let mut note = String::new();
    let has_key = store.get("key").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if store["kdf"].as_str() == Some(MODE_KEYCHAIN) && has_key {
        if keychain_available() {
            let _ = keychain_set(store["key"].as_str().unwrap_or(""));
            note = "主密钥已写入本机钥匙串，导入后可直接使用。".into();
        } else {
            note = "警告：本机没有可用钥匙串，钥匙串模式密库将无法解锁。".into();
        }
        // 主密钥不允许留在磁盘上的密库文件里
        if let Some(o) = store.as_object_mut() {
            o.remove("key");
        }
    } else if store["kdf"].as_str() == Some(MODE_KEYCHAIN) {
        note = "注意：该备份为钥匙串模式且未包含主密钥，导入后本机可能无法解锁（需要原机器的钥匙串）。".into();
    }
    if vault_file().exists() {
        if !force {
            eprintln!("本地已有密库。加 --force 覆盖（旧库会备份为 vault.json.bak）");
            std::process::exit(1);
        }
        let _ = fs::rename(vault_file(), vault_file().with_extension("json.bak"));
    }
    ensure_home();
    if let Err(e) = write_store(&store) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
    audit("IMPORT", json!({ "from_file": file }));
    println!("已导入。{}", note);
    let _ = api("POST", "/lock", Some(&json!({})), Duration::from_secs(1));
}

pub fn cmd_keychain_reset() {
    if !keychain_available() {
        eprintln!("当前环境没有可用的系统钥匙串");
        std::process::exit(1);
    }
    let had = keychain_get();
    keychain_delete();
    audit("KEYCHAIN_RESET", json!({}));
    match had {
        Some(_) => println!("已清除钥匙串中的 keyx 主密钥。"),
        None => println!("钥匙串中没有 keyx 主密钥。"),
    }
}

pub fn cmd_serve() -> ! {
    crate::daemon::serve()
}

// 保留 import 引用，避免未使用告警（部分常量供后续扩展）
#[allow(dead_code)]
fn _unused() {
    let _ = (DEFAULT_ITER, DEFAULT_LOCK_MIN, DEFAULT_PORT, T30, NONINTERACTIVE_HINT);
}
