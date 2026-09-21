//! Key-X Rust 版端到端测试：移植自 Python 版 daemon-test.py（43 项断言）。
//! 覆盖：token 鉴权、创建/解锁/错误密码、密文落盘、set/list、信任流程、run 注入、
//!       scope 隔离、锁定/审计、导出导入、防篡改、解析链、peek、stop、钥匙串模式。

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const PORT: u16 = 7410;

type Counts = (usize, usize);

fn check(c: &mut Counts, name: &str, cond: bool, extra: &str) {
    if cond {
        c.0 += 1;
        println!("  ✓ {}", name);
    } else {
        c.1 += 1;
        println!("  ✗ {}  {}", name, extra);
    }
}

fn http(token: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let url = format!("http://127.0.0.1:{}{}", PORT, path);
    let sent = match (method, body) {
        ("GET", _) => ureq::get(&url).timeout(Duration::from_secs(5)).set("X-KeyX-Auth", token).call(),
        ("DELETE", b) => ureq::delete(&url)
            .timeout(Duration::from_secs(5))
            .set("X-KeyX-Auth", token)
            .send_string(&b.unwrap_or_else(|| json!({})).to_string()),
        (_, b) => ureq::post(&url)
            .timeout(Duration::from_secs(5))
            .set("X-KeyX-Auth", token)
            .send_string(&b.unwrap_or_else(|| json!({})).to_string()),
    };
    match sent {
        Ok(resp) => {
            let s = resp.into_string().unwrap_or_default();
            (200, serde_json::from_str(&s).unwrap_or(Value::Null))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let s = resp.into_string().unwrap_or_default();
            (code, serde_json::from_str(&s).unwrap_or(Value::Null))
        }
        Err(_) => (0, Value::Null),
    }
}

#[test]
fn e2e() {
    let bin = env!("CARGO_BIN_EXE_keyx");
    let tmp = std::env::temp_dir().join(format!("keyx-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // /var → /private/var 符号链接：信任记录按 canonicalize 后的路径存储，测试必须用同一路径
    let proj = std::fs::canonicalize(&proj).unwrap();

    let base: Vec<(String, String)> = vec![
        ("KEYX_HOME".into(), home.to_string_lossy().to_string()),
        ("KEYX_PORT".into(), PORT.to_string()),
        ("KEYX_PASSWORD".into(), "test-master-2026!".into()),
        ("KEYX_MODE".into(), "password".into()),
        ("KEYX_AUTOLOCK_MIN".into(), "30".into()),
        ("KEYX_ITERATIONS".into(), "1000".into()),
        ("KEYX_PEEK_TEST".into(), "1".into()),
    ];

    let cli = |cwd: &std::path::Path, args: &[&str], input: Option<&str>| -> Output {
        let mut c = Command::new(bin);
        c.args(args).current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in &base {
            c.env(k, v);
        }
        match input {
            Some(inp) => {
                c.stdin(Stdio::piped());
                let mut child = c.spawn().expect("spawn cli");
                let _ = child.stdin.as_mut().unwrap().write_all(inp.as_bytes());
                child.wait_with_output().expect("wait cli")
            }
            None => {
                c.stdin(Stdio::null());
                c.output().expect("run cli")
            }
        }
    };

    // ── 启动 daemon ──
    let log = std::fs::File::create(tmp.join("daemon.log")).unwrap();
    let mut dcmd = Command::new(bin);
    dcmd.arg("serve").stdout(log.try_clone().unwrap()).stderr(log);
    for (k, v) in &base {
        dcmd.env(k, v);
    }
    let mut daemon = dcmd.spawn().expect("spawn daemon");
    let token_path = home.join("client.token");
    let mut token = String::new();
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(t) = std::fs::read_to_string(&token_path) {
            let t = t.trim().to_string();
            if t.len() >= 32 && http(&t, "GET", "/status", None).0 == 200 {
                token = t;
                break;
            }
        }
    }
    let mut c: Counts = (0, 0);
    if token.is_empty() {
        let _ = daemon.kill();
        let _ = std::fs::remove_dir_all(&tmp);
        panic!("daemon 未启动");
    }

    // ── [1] 无授权请求被拒绝 ──
    println!("\n[1] 无授权请求被拒绝");
    let (st, _) = http("", "GET", "/status", None);
    check(&mut c, "缺 client token 请求被拒", st == 403, &format!("st={}", st));

    // ── [2] 创建 / 错误密码 / 解锁 ──
    println!("\n[2] 创建 / 错误密码 / 解锁");
    let (st, _) = http(&token, "POST", "/create", Some(json!({ "password": "test-master-2026!" })));
    check(&mut c, "创建密码库", st == 200, &format!("st={}", st));
    let (st, r) = http(&token, "POST", "/unlock", Some(json!({ "password": "wrong-password" })));
    check(&mut c, "错误主密码 401", st == 401 && r["error"] == "WRONG_PASSWORD", &format!("{} {}", st, r));
    let (st, _) = http(&token, "POST", "/unlock", Some(json!({ "password": "test-master-2026!" })));
    check(&mut c, "正确主密码解锁", st == 200, &format!("st={}", st));

    // ── [3] 存入 → key-id → 磁盘密文 ──
    println!("\n[3] 存入 → key-id → 磁盘密文");
    let out = cli(&proj, &["set", "订单库", "--username", "order_app"], None);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let kid = stdout
        .lines()
        .find(|l| l.contains("key-id:"))
        .map(|l| l.split("key-id:").nth(1).unwrap().split_whitespace().next().unwrap().to_string())
        .unwrap_or_default();
    check(&mut c, "set 输出 key-id", kid.starts_with("kx_"), &format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr)));
    let vault_raw = std::fs::read_to_string(home.join("vault.json")).unwrap_or_default();
    check(&mut c, "磁盘无明文密码（set 走隐藏输入，测试中为空）", !vault_raw.contains("order_app"), "");
    let (st, r2) = http(&token, "POST", "/entries", Some(json!({ "name": "带密码的条目", "password": "Real-Secret-981!" })));
    let kid2 = r2["id"].as_str().unwrap_or("").to_string();
    check(&mut c, "API 存入密码得到 key-id", st == 200 && kid2.starts_with("kx_"), &format!("{} {}", st, r2));
    let vault_raw = std::fs::read_to_string(home.join("vault.json")).unwrap_or_default();
    check(&mut c, "密库文件不含明文", !vault_raw.contains("Real-Secret-981!"), "");
    let out = cli(&proj, &["list"], None);
    let list_out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "list 展示 key-id 不展示密码", list_out.contains(&kid2) && !list_out.contains("Real-Secret-981!"), &list_out);

    // ── [4] 信任流程 + run 注入 ──
    println!("\n[4] 信任流程 + run 注入");
    let (st, r) = http(&token, "POST", "/resolve", Some(json!({ "project": proj.to_string_lossy(), "keys": [kid2] })));
    check(
        &mut c,
        "未信任项目 resolve → 403 NEED_TRUST",
        st == 403 && r["error"] == "NEED_TRUST" && r["need"] == json!([kid2]),
        &format!("{} {}", st, r),
    );
    let out = cli(
        &proj,
        &["run", "--env", &format!("DB_PASSWORD={}", kid2), "--", "python3", "-c", "import os;print(os.environ['DB_PASSWORD'],end='')"],
        Some("n\n"),
    );
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "未信任时 run 进入询问，输入 n 取消", out.status.code() == Some(1) && all.contains("已取消"), &all);
    let out = cli(
        &proj,
        &["run", "--env", &format!("DB_PASSWORD={}", kid2), "--", "python3", "-c", "import os;print(os.environ['DB_PASSWORD'],end='')"],
        Some("y\n"),
    );
    check(
        &mut c,
        "run 注入环境变量，子进程拿到明文",
        String::from_utf8_lossy(&out.stdout) == "Real-Secret-981!",
        &format!("{:?}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );
    let (st, r) = http(&token, "POST", "/resolve", Some(json!({ "project": proj.to_string_lossy(), "keys": [kid2] })));
    check(
        &mut c,
        "授权后 daemon 直接解析",
        st == 200 && r["passwords"][&kid2] == "Real-Secret-981!",
        &format!("{} {}", st, r),
    );
    let (st, _) = http(&token, "POST", "/resolve", Some(json!({ "project": proj.to_string_lossy(), "keys": ["kx_nope"] })));
    check(&mut c, "未知 key-id → 404", st == 404, &format!("st={}", st));

    // ── [5] 其他项目无权访问（scope 隔离）──
    println!("\n[5] 其他项目无权访问（scope 隔离）");
    let other = tmp.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let out = cli(
        &other,
        &["run", "--env", &format!("PW={}", kid2), "--", "python3", "-c", "print(1)"],
        Some("n\n"),
    );
    check(&mut c, "其他项目首次访问走询问，拒绝后无法解析", out.status.code() == Some(1), &format!("st={:?}", out.status.code()));

    // ── [6] 锁定 / 解锁 / 审计 ──
    println!("\n[6] 锁定 / 解锁 / 审计");
    let _ = http(&token, "POST", "/lock", None);
    let (st, r) = http(&token, "POST", "/resolve", Some(json!({ "project": proj.to_string_lossy(), "keys": [kid2] })));
    check(&mut c, "锁定后 resolve → 423", st == 423 && r["error"] == "LOCKED", &format!("{} {}", st, r));
    let _ = http(&token, "POST", "/unlock", Some(json!({ "password": "test-master-2026!" })));
    let audit = std::fs::read_to_string(home.join("audit.log")).unwrap_or_default();
    check(&mut c, "审计含 RESOLVE", audit.contains("\"event\": \"RESOLVE\"") || audit.contains("\"event\":\"RESOLVE\""), "");
    check(&mut c, "审计含 DENY(未信任)", audit.contains("not_trusted"), "");

    // ── [7] 导出 / 导入 ──
    println!("\n[7] 导出 / 导入（浏览器备份格式互通）");
    let backup = home.join("backup.json");
    let _ = cli(&proj, &["export", "-o", &backup.to_string_lossy()], None);
    check(&mut c, "导出成功", backup.exists(), "");
    let bk = std::fs::read_to_string(&backup).unwrap_or_default();
    check(&mut c, "导出为密文", !bk.contains("Real-Secret-981!"), "");
    let _ = std::fs::remove_file(home.join("vault.json"));
    let out = cli(&proj, &["import", &backup.to_string_lossy()], None);
    let imp_out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "导入成功", imp_out.contains("已导入"), &imp_out);
    let out = cli(&proj, &["list"], None);
    let list_out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "导入后可列出（需解锁，测试走环境变量密码）", list_out.contains(&kid2), &list_out);

    // ── [8] 防篡改：KDF 参数被改小 ──
    println!("\n[8] 防篡改：KDF 参数被改小");
    let _ = http(&token, "POST", "/lock", None);
    let mut store: Value = serde_json::from_str(&std::fs::read_to_string(home.join("vault.json")).unwrap()).unwrap();
    let orig_iter = store["iterations"].clone();
    store["iterations"] = json!(1);
    std::fs::write(home.join("vault.json"), store.to_string()).unwrap();
    let out = cli(&proj, &["list"], None);
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "篡改后解锁报 VAULT_CORRUPT", all.contains("VAULT_CORRUPT") || all.contains("篡改"), &all);
    store["iterations"] = orig_iter;
    std::fs::write(home.join("vault.json"), store.to_string()).unwrap();

    // ── [9] rotate：同编号换值 ──
    println!("\n[9] rotate：同编号换值");
    let _ = http(&token, "POST", "/unlock", Some(json!({ "password": "test-master-2026!" })));
    let (st, _) = http(&token, "POST", "/entries", Some(json!({ "id": kid2, "password": "New-Rotated-99!" })));
    check(&mut c, "rotate 更新成功", st == 200, &format!("st={}", st));
    let (st, r) = http(&token, "POST", "/resolve", Some(json!({ "project": proj.to_string_lossy(), "keys": [kid2] })));
    check(
        &mut c,
        "key-id 不变，值已换",
        st == 200 && r["passwords"][&kid2] == "New-Rotated-99!",
        &format!("{} {}", st, r),
    );
    let audit = std::fs::read_to_string(home.join("audit.log")).unwrap_or_default();
    check(&mut c, "审计含 ROTATE", audit.contains("ROTATE"), "");
    let (st, r) = http(&token, "POST", "/entries", Some(json!({ "name": "Redis", "password": "Other-456!" })));
    let kid3 = r["id"].as_str().unwrap_or("").to_string();
    check(&mut c, "第三条条目就绪", st == 200 && kid3.starts_with("kx_"), &format!("{} {}", st, r));
    let _ = http(&token, "POST", "/trust", Some(json!({ "project": proj.to_string_lossy(), "keys": [kid3] })));

    // ── [10] keyx run 解析链 ──
    println!("\n[10] keyx run 解析链：.env / .keyx.toml / 显式");
    std::fs::write(proj.join(".env"), format!("ENV_PW={}\nPLAIN=hello\n", kid2)).unwrap();
    let _ = std::fs::remove_file(proj.join(".keyx.toml"));
    let out = cli(
        &proj,
        &["run", "--env", &format!("OUT_PW={}", kid2), "--", "python3", "-c", "import os;print(os.environ['OUT_PW'],os.environ['ENV_PW'],end='')"],
        None,
    );
    check(
        &mut c,
        "显式 --env 与 .env kx_* 引用同时注入",
        String::from_utf8_lossy(&out.stdout) == "New-Rotated-99! New-Rotated-99!",
        &format!("{:?}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );
    std::fs::write(proj.join(".keyx.toml"), format!("[env]\nENV_PW = \"{}\"\n", kid3)).unwrap();
    let out = cli(&proj, &["run", "--", "python3", "-c", "import os;print(os.environ['ENV_PW'],end='')"], None);
    check(
        &mut c,
        ".keyx.toml [env] 优先于 .env",
        String::from_utf8_lossy(&out.stdout) == "Other-456!",
        &format!("{:?}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );
    let out = cli(
        &proj,
        &["run", "--env", &format!("ENV_PW={}", kid2), "--", "python3", "-c", "import os;print(os.environ['ENV_PW'],end='')"],
        None,
    );
    check(
        &mut c,
        "显式 --env 优先于 .keyx.toml",
        String::from_utf8_lossy(&out.stdout) == "New-Rotated-99!",
        &format!("{:?}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );
    let out = cli(&other, &["run", "--", "python3", "-c", "print('passthrough',end='')"], None);
    check(
        &mut c,
        "无引用目录纯透传",
        String::from_utf8_lossy(&out.stdout) == "passthrough",
        &format!("{:?}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );

    // ── [11] 非交互解锁死局提示 ──
    println!("\n[11] 非交互解锁死局提示");
    let _ = http(&token, "POST", "/lock", None);
    let mut ccmd = Command::new(bin);
    ccmd.args(["list"]).current_dir(&proj).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    for (k, v) in &base {
        if k != "KEYX_PASSWORD" {
            ccmd.env(k, v);
        }
    }
    let out = ccmd.output().expect("list without password");
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "非交互锁定 → 提示人工解锁而非密码错误", all.contains("keyx unlock"), &all);

    // ── [12] keyx peek（测试模式跳过 GUI）──
    println!("\n[12] keyx peek（弹窗人工确认；测试模式跳过 GUI）");
    let out = cli(&proj, &["peek", "kx_nope"], None);
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "peek 未知编号 → 报错退出", !out.status.success() && all.contains("没有找到"), &all);
    let out = cli(&proj, &["peek", &kid2], None);
    check(
        &mut c,
        "peek 按编号 → 确认",
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("已确认"),
        &format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );
    let out = cli(&proj, &["peek", "带密码的条目"], None);
    check(
        &mut c,
        "peek 按名称 → 确认",
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("已确认"),
        &format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
    );
    let audit = std::fs::read_to_string(home.join("audit.log")).unwrap_or_default();
    check(&mut c, "审计记录 PEEK", audit.contains("PEEK"), "");

    // ── [13] keyx stop ──
    println!("\n[13] keyx stop");
    let out = cli(&proj, &["stop"], None);
    let stop_out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(&mut c, "stop 成功", stop_out.contains("已停止"), &stop_out);
    let (st, _) = http(&token, "GET", "/status", None);
    check(&mut c, "停止后 daemon 不可达", st == 0, &format!("st={}", st));

    // ── [14] 钥匙串模式（仅 macOS 真机验证）──
    // 注意：所有钥匙串交互必须经 keyx 二进制自身完成（同二进制 ACL 一致，不会触发
    // macOS 授权弹窗）；测试进程直接 get_password 会因 ACL 不匹配被 SecKeychain 挂起。
    // 条目用户名按进程随机化：避免残留、避免跨构建读取。
    #[cfg(target_os = "macos")]
    {
        println!("\n[14] 钥匙串模式");
        let kc_home = tmp.join("kc");
        let kc_cwd = tmp.join("kccwd");
        std::fs::create_dir_all(&kc_cwd).unwrap();
        let kc_port: u16 = 7420 + (std::process::id() % 80) as u16;
        let kc_user = format!("keyx-selftest-{}", std::process::id());
        let kcli = |args: &[&str], input: Option<&str>| -> Output {
            let mut c2 = Command::new(bin);
            c2.args(args).current_dir(&kc_cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
            c2.env("KEYX_HOME", &kc_home).env("KEYX_PORT", kc_port.to_string())
                .env("KEYX_MODE", "keychain").env("KEYX_KEYCHAIN_USER", &kc_user)
                .env("KEYX_SECRET", "Kc-Secret-77!");
            match input {
                Some(inp) => {
                    c2.stdin(Stdio::piped());
                    let mut child = c2.spawn().expect("spawn kcli");
                    let _ = child.stdin.as_mut().unwrap().write_all(inp.as_bytes());
                    child.wait_with_output().expect("wait kcli")
                }
                None => {
                    c2.stdin(Stdio::null());
                    c2.output().expect("run kcli")
                }
            }
        };
        let out = kcli(&["set", "钥匙串条目"], None);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let kid4 = stdout
            .lines()
            .find(|l| l.contains("key-id:"))
            .map(|l| l.split("key-id:").nth(1).unwrap().split_whitespace().next().unwrap().to_string())
            .unwrap_or_default();
        check(
            &mut c,
            "钥匙串模式 set 成功（无任何密码输入）",
            kid4.starts_with("kx_"),
            &format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr)),
        );
        if kid4.is_empty() {
            println!("  - set 失败，跳过后续钥匙串用例");
        } else {
            let kc_store: Value = serde_json::from_str(&std::fs::read_to_string(kc_home.join("vault.json")).unwrap()).unwrap();
            check(&mut c, "vault kdf=KEYCHAIN", kc_store["kdf"] == "KEYCHAIN", &format!("{}", kc_store["kdf"]));
            // 二进制间接验证钥匙串里有主密钥：status 应显示已解锁（解锁走钥匙串）
            let out = kcli(&["status"], None);
            let st_out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            check(&mut c, "主密钥可从钥匙串解锁（status 已解锁）", st_out.contains("已解锁"), &st_out);
            let raw = std::fs::read_to_string(kc_home.join("vault.json")).unwrap_or_default();
            let has_key_field = serde_json::from_str::<Value>(&raw).map(|v| v.get("key").is_some()).unwrap_or(false);
            check(&mut c, "密库文件不含主密钥", !has_key_field, "");
            let _ = kcli(&["trust", "add", &kid4], None);
            let out = kcli(
                &["run", "--env", &format!("KC_PW={}", kid4), "--", "python3", "-c", "import os;print(os.environ['KC_PW'],end='')"],
                None,
            );
            check(
                &mut c,
                "钥匙串模式 run 注入",
                String::from_utf8_lossy(&out.stdout) == "Kc-Secret-77!",
                &format!("{:?}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
            );
        }
        let _ = kcli(&["keychain-reset"], None); // 经二进制清理本次条目
        let _ = kcli(&["stop"], None);
    }

    // ── 清理 ──
    let _ = daemon.kill();
    let _ = daemon.wait();
    let _ = std::fs::remove_dir_all(&tmp);

    println!("\n结果：{} 通过，{} 失败", c.0, c.1);
    assert!(c.1 == 0, "有 {} 项断言失败", c.1);
}
