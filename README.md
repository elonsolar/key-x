# Key-X

密码只存一次。之后你写代码、推 git、跟 AI 聊天的地方，只会看到 `kx_` 编号。

真值锁在本机加密密库里，程序启动那一刻由 `keyx run` 现场换出来、塞进环境变量——你的代码原来怎么读环境变量，现在还怎么读，一行不用改。

## 三十秒上手

```bash
keyx set openai-prod        # 终端里输入密码（隐藏输入），得到 kx_8f2a1c9d
```

配置里写编号（这一步通常是 AI 干的）：

```toml
# .keyx.toml
[env]
OPENAI_API_KEY = "kx_8f2a1c9d"
```

启动：

```bash
keyx run npm run dev
```

就这样。明文只在你输入的那一刻存在，之后它不在 .env 里、不在 git 里、不在 AI 对话里。

## 之后的日子

平时你什么都不用做——启动是 AI 带着前缀跑的，你感知不到 keyx 的存在。偶尔几件事：

- **换密码**：`keyx rotate kx_8f2a1c9d`，编号不变，配置零改动；
- **想取回密码**（比如贴到 Vercel）：`keyx peek kx_8f2a1c9d`，系统弹窗确认，值只进剪贴板，45 秒自动清空；
- **密钥泄漏了？** 别慌，轮换只要 60 秒：源头发新值 → `keyx set` 存新值 → 旧值作废。泄漏出去的旧钥匙从此是废铁。

## 装

```bash
curl -fsSL https://raw.githubusercontent.com/elonsolar/key-x/main/install.sh | bash    # macOS / Linux
irm  https://raw.githubusercontent.com/elonsolar/key-x/main/install.ps1 | iex          # Windows
```

没有注册，没有账号，没有要记的密码——密库跟你的系统登录走（macOS 钥匙串 / Windows 凭据管理器），首次 `keyx set` 自动建库。没有钥匙串的服务器环境会回退主密码模式。

## 给 AI 用

Key-X 的用法准则（永不向 AI 索要明文、AI 看到明文立即引导轮换等）做成了技能，AI 装上它才会主动带你走这套流程：

```bash
npx skills add elonsolar/key-x-skill
```

用 Codex / OpenCode / Cursor 这类没有技能机制的引擎？把仓库里的 `AGENTS-SNIPPET.md` 粘进项目的 `AGENTS.md` 就行。

## 它防什么，不防什么

**防**：明文进 .env、进 git、进 AI 对话转录、进日志。就算泄漏发生了，损失也被压缩成一次 60 秒的轮换。

**不防**：已经攻陷你电脑的人（同用户的恶意进程能读到一切）；应用自己把密码打印进报错日志。丑话说在前面，不卖幻觉。

## 平台

macOS（intel / arm64）全流程真机验证过。Linux 和 Windows 的二进制由 CI 构建，代码路径在、但没上过真机——我们不说没验证过的话。

## 开发

```bash
cargo test    # 43 项端到端断言
```
