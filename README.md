# Key-X · 本机密钥引用系统

> 一句话：密码只存进 keyx 一次，之后配置、git、AI 对话里永远只有 `kx_` 编号；启动程序时 `keyx run` 把编号现场换成真值注入环境变量。

## 密钥状态公理

**明文一旦出现在文件、对话或任何有副本的地方，即视为已泄漏**——不可撤回，唯一补救是轮换（在源头发新值、旧值作废）。本系统围绕一条规则设计：明文只存在于"签发它的系统"和"使用它的进程内存"，其余位置只允许出现 `kx_` 引用。

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/elonsolar/key-x/main/install.sh | bash    # macOS / Linux（自动挑架构）
irm  https://raw.githubusercontent.com/elonsolar/key-x/main/install.ps1 | iex          # Windows（amd64）
```

安装脚本只做三件事：下载对应架构的二进制、放进用户级 PATH 目录、打印版本确认。不碰系统目录、无需 sudo。

首次使用无需注册任何账号密码：第一条 `keyx set` 自动创建密库（默认钥匙串模式，跟随系统登录）。

## 使用（三个动作）

```bash
# 1) 存：首次自动创建密库
keyx set openai-prod --notes "生产环境，公司主账号"
# → key-id: kx_3febffc7

# 2) 声明引用（写入项目 .keyx.toml 或 .env，通常由 AI 代劳）
cat > .keyx.toml <<'EOF'
[env]
OPENAI_API_KEY = "kx_3febffc7"
EOF

# 3) 启动：自动解析全部引用，明文只进子进程环境变量
keyx run npm run dev
```

解析优先级：显式 `--env VAR=kx_id` > `.keyx.toml [env]` > `.env` 中值为 `kx_*` 的条目 > 纯透传（无引用时 keyx 相当于不存在）。

> `.env` 兼容路径注意：若应用以 `dotenv({override:true})` 方式加载 .env，kx_ 字面量会覆盖注入值——这类项目请用 `.keyx.toml`。

## 命令

```bash
keyx set <名称>               # 存入 → 输出 kx_ 编号
keyx list                     # 列表（无明文）
keyx peek <编号或名称>         # 弹窗查看/复制：人工点按确认，值只进剪贴板（45 秒自动清空）或弹窗本身，不进终端输出
keyx rotate kx_xxx            # 轮换：同编号换值，引用/配置零改动
keyx run <命令>               # 解析引用并注入环境变量后启动
keyx trust list|add|rm        # 项目授权记账
keyx audit -n 20              # 操作留痕
keyx status|lock|unlock|stop  # daemon 生命周期
keyx export -o f.json [--include-key]   # 备份；--include-key 仅钥匙串模式需要（文件等同明文！）
keyx import f.json            # 导入（覆盖本地，旧库自动备份）
keyx rm kx_xxx                # 删除
keyx keychain-reset           # 清除钥匙串主密钥（确认旧密库废弃后使用）
```

## 两种解锁模式

| | 钥匙串模式（默认） | 主密码模式 |
|---|---|---|
| 主密钥 | 随机生成，存 macOS 钥匙串 / Windows 凭据管理器 / Linux SecretService | 用户主密码 PBKDF2(600k) 派生 |
| 用户负担 | **零**：无密码可设、可忘、可输 | 记住主密码（丢失 = 数据不可恢复） |
| 适用 | 个人电脑（产品默认） | headless 服务器 / SSH |
| 选择 | 自动（有钥匙串即用） | `KEYX_MODE=password`，或环境无钥匙串时自动回退 |

跨机迁移：主密码模式直接 export/import；钥匙串模式需 `keyx export --include-key`（导出文件含主密钥，**等同明文**）。

## AI agent 集成

本工具的行为准则（永不索要明文、写入前转向、识别即报警等）由配套 skill 承载，独立发行：

```bash
npx skills add elonsolar/key-x-skill
```

无 skills 机制的引擎（Codex/OpenCode/Cursor 等）：把该仓库里的 `AGENTS-SNIPPET.md` 粘进项目 `AGENTS.md` / `.cursorrules`。

## 平台支持（诚实版）

| | macOS arm64/intel | Linux amd64/arm64 | Windows amd64 |
|---|---|---|---|
| 二进制 | CI 原生构建 | CI 构建（arm64 交叉） | CI 原生构建 |
| 零密码解锁（系统钥匙串） | ✅ Keychain（真机验证） | 需 SecretService（未真机验证） | 凭据管理器（未真机验证） |
| peek 弹窗 | ✅ 原生弹窗（真机验证） | 降级：直接进剪贴板 | 降级：直接进剪贴板 |
| 回退 | 无钥匙串环境自动回退主密码模式（三平台一致） | 同左 | 同左 |

不承诺没测过的东西。

## 安全模型与边界（诚实版）

- **加密**：AES-256-GCM；钥匙串模式主密钥由 OS 保护、盐参与认证（AAD 绑定文件）；主密码模式 PBKDF2-SHA256 600k 次派生。KDF 参数参与密文校验，文件被篡改（如改小迭代数）解锁时报 `VAULT_CORRUPT`。
- **边界**：daemon 仅监听 `127.0.0.1` + client token。**本系统防的是"配置/git/转录/日志里的明文泄漏"，不防同用户的本机恶意进程**（它可读 client.token 与钥匙串）。
- **措辞声明**：`trust` 是授权记账（同用户进程可绕过，用于防误操作 + 留痕），不是访问控制；`audit` 是操作留痕（同用户可改写），不是取证日志。
- **防降级**：已有主密钥但无密库文件时拒绝覆盖创建（`KEYCHAIN_OCCUPIED`），需显式 `keyx keychain-reset`。
- **运行期输出**：应用自身报错打印的连接串/密钥不经过 keyx，工具层不脱敏——这条通道不在覆盖范围内。

## 开发

```bash
cargo test        # 43 项端到端断言
cargo build --release
```

测试注意：

- dev 依赖带 `opt-level = 2`，否则 pbkdf2 在 debug 下极慢；测试用 `KEYX_ITERATIONS=1000` 提速。
- 钥匙串用例只能在 macOS 跑，且**所有钥匙串交互必须经 keyx 二进制自身**：测试进程直接读
  钥匙串会因 ACL 不匹配触发系统授权弹窗并挂起。
- Windows/Linux 路径未真机验证。

## 发布

推 tag（`v*`）触发 GitHub Actions 在 5 个原生 runner 构建，产物自动挂到 Release。首次发布前确认 `install.sh`/`install.ps1` 中的 `elonsolar/key-x` 与实际仓库一致。
