# Key-X

## 解决什么问题

写代码的人手里都攥着一把明文密码：OpenAI 的 key、数据库密码、各种 token。它们散落在 `.env` 里、被推上 git、被粘进 AI 对话、被打印进日志——每一个副本都是一次泄漏事故的种子。

Key-X 把真值收进本机加密密库，之后所有地方只出现 `kx_` 编号。泄漏了编号等于什么都没泄；真要泄漏了密码，换一把新的只要 60 秒。

## 面向谁

**AI 原生的开发者**——尤其是让 AI 代写代码、自己不看代码的人。整套流程里你只需要做一件事：在终端里输入一次密码。剩下的（写配置、启动、轮换、排查）都是 AI 的活。

也适合任何讨厌在 `.env` 里养明文的人。不需要改一行应用代码：程序原来怎么读环境变量，现在还怎么读。

## 怎么做的

密码存进本机的 AES-256-GCM 加密密库（默认跟你的系统登录走，无需记任何密码）。
项目配置里写的不是密码，是 `kx_` 编号。
`keyx run` 启动程序时把编号现场换成真值、注入环境变量——明文只存在于"你输入的那一刻"和"程序进程的内存里"。

## 四个操作（参考）

```bash
keyx set openai-prod        # 设置密码：终端隐藏输入，得到 kx_ 编号（首次自动建库，无需注册）
keyx peek kx_8f2a1c9d       # 查看/找回密码：系统弹窗确认，值只进剪贴板，45 秒自动清空（SSH/无桌面：人工终端回车确认后仅复制；AI/脚本调用拒绝）
keyx rotate kx_8f2a1c9d     # 更换密码：同编号换值，配置零改动；记得去源头撤销旧值
keyx run npm run dev        # 启动程序：编号现场换真值，注入环境变量
```

忘了自己配过哪些？`keyx list` 全部列出（无明文）。

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/elonsolar/key-x/main/install.sh | bash    # macOS / Linux
irm  https://raw.githubusercontent.com/elonsolar/key-x/main/install.ps1 | iex          # Windows
```

## 配套技能

本工具的使用准则由 AI 技能承载——**怎么装、什么场景自动触发、老项目怎么排查泄漏、新项目 AI 会怎么做，见 [key-x-skill](https://github.com/elonsolar/key-x-skill)**：

```bash
npx skills add elonsolar/key-x-skill
```

## 发版流程

发新版时按此清单执行，不要临场发挥：

1. **版本号两处对齐**：`Cargo.toml` 的 `version` 和 `src/core.rs` 的 `VERSION` 必须一致（client 靠它和 daemon 做版本握手），改完 `cargo check` 同步 Cargo.lock。
2. **提交并推送 main**。
3. **打 tag 触发发布**：`git tag vX.Y.Z && git push origin vX.Y.Z`（tag 不会跟着 `git push` 走，要单独推）。推 tag = 对外发版。
4. CI（`.github/workflows/release.yml`）自动在五平台构建、挂产物到 GitHub Release；进度看仓库 Actions 页。
5. 某平台挂了：修复后删 tag 重来（`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`），或在 Actions 页重跑失败的 job。
6. **发完验证**：Releases 页面出现 5 个附件；新机器跑一遍安装脚本确认能装上。

发布铁律（首跑踩过的坑，别改回去）：

- Windows 打包用 `Compress-Archive`——GitHub 的 Windows runner 上没有 `zip` 命令。
- Linux job 必须装 `libdbus-1-dev`——keyring 的 secret-service 功能走 dbus，缺头文件构建直接失败。
- Linux arm64 用原生 arm runner（`ubuntu-22.04-arm`）构建，不要改回 x86 交叉编译（dbus 交叉依赖是大坑）。
- `install.sh` / `install.ps1` 的资产命名（`keyx-<target>.tar.gz|zip`）和 workflow 的产物名严格对齐，两边改动必须同步。
