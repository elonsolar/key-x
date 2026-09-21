# Key-X 一键安装（Windows）：从 GitHub Releases 下载二进制并加入用户 PATH
# 用法: irm <安装脚本URL> | iex
$ErrorActionPreference = "Stop"

$repo = if ($env:KEYX_REPO) { $env:KEYX_REPO } else { "elonsolar/key-x" }   # TODO: 发布时替换
$dir  = if ($env:KEYX_INSTALL_DIR) { $env:KEYX_INSTALL_DIR } else { "$env:USERPROFILE\.local\bin" }
$arch = "x86_64"   # 当前仅发布 64 位 Windows
$url  = "https://github.com/$repo/releases/latest/download/keyx-$arch-pc-windows-msvc.zip"

Write-Host "下载 $url"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Invoke-WebRequest -Uri $url -OutFile "$tmp\keyx.zip"
Expand-Archive -Path "$tmp\keyx.zip" -DestinationPath $tmp -Force

New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item "$tmp\keyx.exe" "$dir\keyx.exe" -Force
Remove-Item -Recurse -Force $tmp

& "$dir\keyx.exe" --version

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$dir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dir", "User")
    Write-Host "已加入用户 PATH（重开终端生效）"
}
Write-Host "完成。下一步：对 AI 说『帮我用 keyx 管理密码』，或在终端运行 keyx set <名称>"
