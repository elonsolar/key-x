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
keyx peek kx_8f2a1c9d       # 查看/找回密码：系统弹窗确认，值只进剪贴板，45 秒自动清空
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
