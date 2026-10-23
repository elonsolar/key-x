// 自更新：查询 GitHub Releases、下载校验替换二进制。
// 只替换本机可执行文件并重启 daemon，不碰密库与钥匙串。
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::core::{audit, home_dir, VERSION};

fn repo() -> String {
    std::env::var("KEYX_REPO").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "elonsolar/key-x".into())
}

pub fn update_check_disabled() -> bool {
    std::env::var("KEYX_NO_UPDATE_CHECK").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false)
}

/// 当前平台对应的 Release 资产名（与 release.yml 打包名严格对齐，改动两边必须同步）
fn target_asset() -> Option<String> {
    let (os, ext) = match std::env::consts::OS {
        "macos" => ("apple-darwin", "tar.gz"),
        "linux" => ("unknown-linux-gnu", "tar.gz"),
        "windows" => ("pc-windows-msvc", "zip"),
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(format!("keyx-{}-{}.{}", arch, os, ext))
}

fn strip_v(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// 语义化版本比较：other > cur 才提示/升级（防本地构建版本超前于 Release 时误报）
pub fn is_newer(other: &str, cur: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        strip_v(s).split('.').map(|p| p.parse().unwrap_or(0)).collect::<Vec<_>>()
    };
    let (a, b) = (parse(other), parse(cur));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// 弱网重试：GitHub 在部分网络下连接间歇性失败，单次失败直接放弃会让更新基本不可用
fn with_retry<T>(times: u32, f: impl Fn() -> Result<T, String>) -> Result<T, String> {
    let mut last = String::new();
    for i in 0..times {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(3));
        }
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 查询 GitHub 最新 Release：返回 (tag, 资产名→下载地址)
pub fn latest_release() -> Result<(String, Vec<(String, String)>), String> {
    with_retry(2, || {
        let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
        let resp = ureq::get(&url)
            // 慢网络（代理/跨境）仅建连就可能 7s+，API 查询给足 15s
            .timeout(Duration::from_secs(15))
            .set("User-Agent", "keyx-update")
            .call()
            .map_err(|e| format!("{}", e))?;
        let body: Value = serde_json::from_str(&resp.into_string().map_err(|e| format!("{}", e))?)
            .map_err(|e| format!("{}", e))?;
        let tag = body["tag_name"].as_str().unwrap_or("").to_string();
        if tag.is_empty() {
            return Err("Release 信息里没有 tag".into());
        }
        let assets = body["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((tag, assets))
    })
}

fn http_download(url: &str, dest: &PathBuf) -> Result<(), String> {
    // 下载 3 次重试：github.com → objects.githubusercontent.com 任一跳抖动都算失败
    with_retry(3, || {
        let mut reader = ureq::get(url)
            // 资产 ~1.5MB，慢网络 24KB/s 也要 60s+；600s 上限防呆挂
            .timeout(Duration::from_secs(600))
            .set("User-Agent", "keyx-update")
            .call()
            .map_err(|e| format!("{}", e))?
            .into_reader();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map_err(|e| format!("{}", e))?;
        std::fs::write(dest, &buf).map_err(|e| format!("{}", e))?;
        Ok(())
    })
}

/// 就地替换正在运行的二进制：unix 先拷到同目录再 rename 覆盖（原子）；
/// Windows 不能覆盖运行中的 exe，改名旧的再放新的。
/// staged 名带 pid：手动 update 与 daemon 自动升级并发时互不踩踏
fn replace_binary(new_bin: &PathBuf) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("{}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let staged = exe.with_extension(format!("keyx.new.{}", std::process::id()));
        std::fs::copy(new_bin, &staged).map_err(|e| format!("写入 {} 失败：{}", staged.display(), e))?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}", e))?;
        std::fs::rename(&staged, &exe).map_err(|e| {
            format!(
                "覆盖 {} 失败：{}（目录不可写时手动执行: sudo install -m 755 {} {}）",
                exe.display(), e, new_bin.display(), exe.display()
            )
        })
    }
    #[cfg(windows)]
    {
        let old = exe.with_extension(format!("exe.{}.old", std::process::id()));
        let _ = std::fs::rename(&exe, &old);
        std::fs::copy(new_bin, &exe).map_err(|e| format!("覆盖 {} 失败：{}", exe.display(), e))
    }
}

/// 下载 → 解压 → 校验 → 原地替换二进制。成功即已替换；daemon 侧的调用方
/// 负责退出进程，CLI 侧负责停/拉 daemon
pub fn perform_update(tag: &str, assets: &[(String, String)]) -> Result<(), String> {
    let asset = match target_asset().and_then(|want| assets.iter().find(|(n, _)| *n == want).cloned()) {
        Some((_, url)) => url,
        None => {
            let want = target_asset().unwrap_or_default();
            return Err(format!("Release {} 缺少本平台资产 {}（改名需与 release.yml 同步）", tag, want));
        }
    };
    let dl = home_dir().join("download");
    let _ = std::fs::create_dir_all(&dl);
    // Windows Expand-Archive 依赖 .zip 后缀识别格式，命名必须带真实扩展名
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let archive = dl.join(format!("keyx-{}.{}", strip_v(tag), ext));
    println!("下载 {} …", asset);
    http_download(&asset, &archive).map_err(|e| format!("下载失败：{}", e))?;
    let out = dl.join(format!("keyx-{}", strip_v(tag)));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}", e))?;
    // 解压调用系统工具，与 install.sh / release.yml 的打包方式对齐
    #[cfg(unix)]
    let unpack = std::process::Command::new("tar").args(["-xzf", &archive.to_string_lossy(), "-C", &out.to_string_lossy()]).status();
    #[cfg(windows)]
    let unpack = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("Expand-Archive -Path '{}' -DestinationPath '{}' -Force", archive.to_string_lossy(), out.to_string_lossy())])
        .status();
    match unpack {
        Ok(s) if s.success() => {}
        other => return Err(format!("解压失败：{:?}", other)),
    }
    let bin_name = if cfg!(windows) { "keyx.exe" } else { "keyx" };
    let new_bin = out.join(bin_name);
    // 替换前先验证下载到的二进制能跑、且版本与 Release 一致，防止坏资产/半截下载
    let ver = std::process::Command::new(&new_bin).arg("--version").output();
    match ver {
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).contains(strip_v(tag)) => {}
        other => return Err(format!("下载的二进制校验不通过（版本不符或无法运行）：{:?}", other.map(|o| o.status))),
    }
    replace_binary(&new_bin)
}

pub fn cmd_update(check_only: bool) {
    let (tag, assets) = match latest_release() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("检查更新失败：{}（离线或 GitHub 不可达）", e);
            std::process::exit(1);
        }
    };
    if !is_newer(&tag, VERSION) {
        println!("已是最新 v{}。", VERSION);
        return;
    }
    if check_only {
        println!("有新版 {}（当前 v{}）。运行 keyx update 升级。", tag, VERSION);
        return;
    }
    if let Err(e) = perform_update(&tag, &assets) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
    audit("UPDATE", serde_json::json!({ "from": VERSION, "to": tag }));
    // 旧 daemon 还在跑旧逻辑：停掉，下一条命令自动拉起新版
    crate::cmds::cmd_stop();
    crate::client::ensure_daemon();
    println!("已更新到 {}。", tag);
}
