# keyx（Rust 版）· 本机密钥引用系统的发行实现

Python 原型（`../key-x`）的 Rust 重写。密库封装格式逐字节互通：任一侧 `export` 的备份可直接在另一侧 `import`。

## 安装（发布后可用；未发布前先 cargo build --release）

```bash
curl -fsSL <install.sh 地址> | bash          # macOS / Linux（自动挑架构）
irm  <install.ps1 地址> | iex                # Windows（amd64）
```

## 平台支持（诚实版）

| | macOS arm64/intel | Linux amd64/arm64 | Windows amd64 |
|---|---|---|---|
| 二进制 | CI 原生构建 | CI 构建（arm64 交叉） | CI 原生构建 |
| 零密码解锁（系统钥匙串） | ✅ Keychain（真机验证） | 需 SecretService（未真机验证） | 凭据管理器（未真机验证） |
| peek 弹窗 | ✅ 原生弹窗（真机验证） | 降级：直接进剪贴板 | 降级：直接进剪贴板 |
| 回退 | 无钥匙串环境自动回退主密码模式（三平台一致） | 同左 | 同左 |

## 开发

```bash
cargo test        # 43 项端到端断言（自 Python 版 daemon-test.py 移植）
cargo build --release
```

测试注意：

- dev 依赖带 `opt-level = 2`，否则 pbkdf2 在 debug 下极慢；测试用 `KEYX_ITERATIONS=1000` 提速。
- 钥匙串用例只能在 macOS 跑，且**所有钥匙串交互必须经 keyx 二进制自身**：测试进程直接读
  钥匙串会因 ACL 不匹配触发系统授权弹窗并挂起。
- Windows/Linux 路径未真机验证；不承诺未测过的东西。

## 与 Python 版的已知差异

- `keyx status` 输出格式一致；`keyx audit` 事件集合一致（含 PEEK）。
- Python 版 `keychain-reset` 无输出差异；Rust 版在无密钥时打印明确提示。
- daemon 残留检测：Python 用 `os.kill(pid,0)`（僵尸进程误判），Rust 用端口探测（更准）。
# key-x
