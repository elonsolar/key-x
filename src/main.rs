// Key-X · 本机密钥引用系统
mod client;
mod cmds;
mod core;
mod daemon;
mod peek;

use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "Key-X 密钥服务 v{}

用法:
  keyx serve                             启动 daemon（前台，一般由 CLI 自动拉起）
  keyx set <名称> [--username U] [--url U] [--notes N]
                                         存入一条密码 → 输出 kx_ 编号
  keyx list                              列出所有 key-id（无明文）
  keyx peek <编号或名称>                  弹窗查看/复制一条密码（人工确认）
  keyx rotate <key-id> [--username U]    同编号换值（引用零改动）
  keyx rm <key-id>                       删除一条
  keyx run [--env 变量=编号]... -- <命令>  解析引用并注入环境变量后启动
  keyx trust list | add <编号>... [--project 目录] | rm [--project 目录]
  keyx audit [-n N]                      审计留痕
  keyx status | lock | unlock | stop     daemon 生命周期
  keyx export [-o 文件] [--include-key]   导出加密备份
  keyx import <文件> [--force]            导入备份（覆盖本地）
  keyx keychain-reset                    清除钥匙串主密钥",
        core::VERSION
    );
    exit(2)
}

fn need(args: &[String], i: &mut usize, flag: &str) -> String {
    *i += 1;
    match args.get(*i) {
        Some(v) => v.clone(),
        None => {
            eprintln!("{} 需要一个参数", flag);
            exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else { usage() };
    let rest = &args[1..];
    match cmd.as_str() {
        "--version" | "version" => println!("keyx v{}", core::VERSION),
        "serve" => cmds::cmd_serve(),
        "set" => {
            let mut name = None;
            let mut username = None;
            let mut url = None;
            let mut notes = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--username" => username = Some(need(rest, &mut i, "--username")),
                    "--url" => url = Some(need(rest, &mut i, "--url")),
                    "--notes" => notes = Some(need(rest, &mut i, "--notes")),
                    v if v.starts_with('-') => {
                        eprintln!("未知参数 {}", v);
                        exit(2);
                    }
                    v => name = Some(v.to_string()),
                }
                i += 1;
            }
            match name {
                Some(n) => cmds::cmd_set(&n, username.as_deref(), url.as_deref(), notes.as_deref()),
                None => usage(),
            }
        }
        "list" => cmds::cmd_list(rest.first().map(String::as_str)),
        "peek" => match rest.first() {
            Some(k) => cmds::cmd_peek(k),
            None => usage(),
        },
        "rm" => match rest.first() {
            Some(k) => cmds::cmd_rm(k),
            None => usage(),
        },
        "rotate" => {
            let mut key_id = None;
            let mut username = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--username" => username = Some(need(rest, &mut i, "--username")),
                    v if v.starts_with('-') => {
                        eprintln!("未知参数 {}", v);
                        exit(2);
                    }
                    v => key_id = Some(v.to_string()),
                }
                i += 1;
            }
            match key_id {
                Some(k) => cmds::cmd_rotate(&k, username.as_deref()),
                None => usage(),
            }
        }
        "run" => {
            let mut envs: Vec<String> = vec![];
            let mut cmdv: Vec<String> = vec![];
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--env" => envs.push(need(rest, &mut i, "--env")),
                    "--" => {
                        cmdv = rest[i + 1..].to_vec();
                        break;
                    }
                    _ => {
                        // 无 -- 形式：剩余全部是命令
                        cmdv = rest[i..].to_vec();
                        break;
                    }
                }
                i += 1;
            }
            cmds::cmd_run(&envs, &cmdv);
        }
        "trust" => {
            let action = rest.first().map(String::as_str).unwrap_or("");
            let mut keys: Vec<String> = vec![];
            let mut project = None;
            let mut i = 1;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--project" => project = Some(need(rest, &mut i, "--project")),
                    v => keys.push(v.to_string()),
                }
                i += 1;
            }
            cmds::cmd_trust(action, &keys, project.as_deref());
        }
        "audit" => {
            let mut n = 20usize;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "-n" => n = need(rest, &mut i, "-n").parse().unwrap_or(20),
                    v => {
                        eprintln!("未知参数 {}", v);
                        exit(2);
                    }
                }
                i += 1;
            }
            cmds::cmd_audit(n);
        }
        "status" => cmds::cmd_status(),
        "lock" => cmds::cmd_lock(),
        "unlock" => cmds::cmd_unlock(),
        "stop" => cmds::cmd_stop(),
        "export" => {
            let mut out = None;
            let mut include_key = false;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "-o" | "--out" => out = Some(need(rest, &mut i, "-o")),
                    "--include-key" => include_key = true,
                    v => {
                        eprintln!("未知参数 {}", v);
                        exit(2);
                    }
                }
                i += 1;
            }
            cmds::cmd_export(out.as_deref(), include_key);
        }
        "import" => {
            let mut file = None;
            let mut force = false;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--force" => force = true,
                    v if v.starts_with('-') => {
                        eprintln!("未知参数 {}", v);
                        exit(2);
                    }
                    v => file = Some(v.to_string()),
                }
                i += 1;
            }
            match file {
                Some(f) => cmds::cmd_import(&f, force),
                None => usage(),
            }
        }
        "keychain-reset" => cmds::cmd_keychain_reset(),
        _ => usage(),
    }
}
