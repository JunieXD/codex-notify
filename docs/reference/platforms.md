---
title: 支持平台
description: codex-notify 支持的操作系统、处理器架构和后台启动方式。
---

# 支持平台

每个 GitHub Release 都提供五个平台包：

| 平台 | 发行包目标 | 后台启动方式 |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | 当前用户 LaunchAgent |
| macOS Intel | `x86_64-apple-darwin` | 当前用户 LaunchAgent |
| Windows x64 | `x86_64-pc-windows-msvc` | 当前用户登录启动项 |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | systemd 用户服务 |
| Linux x64 | `x86_64-unknown-linux-gnu` | systemd 用户服务 |

安装器会自动识别操作系统和处理器架构，不需要手动选择文件。

## 通用要求

- Codex CLI 或 ChatGPT App 中的 Codex 已经可以正常使用；
- 当前用户可以访问自己的 Codex 配置目录；
- 可以连接飞书开放平台和 GitHub Releases；
- 安装和运行不需要管理员权限。

## macOS

支持 Apple Silicon 和 Intel。配置与 App Secret 保存在 `~/.codex-notify/config.toml`，后台 watcher 使用当前用户的 LaunchAgent 登录自启。

如果系统首次运行下载的程序时弹出安全提示，请确认文件来自项目 GitHub Release，并按照 macOS 提示允许运行。

从旧版本首次升级时，系统还可能询问钥匙串权限。请确认授权对象是 macOS 自带的 `/usr/bin/security` 并选择“始终允许”；完成一次迁移后，后续版本无需重复授权。

## Windows

支持 x64 Windows 10/11。配置与 App Secret 保存在用户目录下的 `.codex-notify/config.toml`，watcher 使用当前用户登录启动项，不写入系统级服务。

安装后若找不到 `codex-notify`，请按照安装脚本提示把安装目录加入用户 `PATH`，再重新打开 PowerShell。

## Linux

支持 ARM64 和 x64，要求：

- 使用 systemd 用户会话；
- 系统包含常见的 glibc 运行环境。

Ubuntu 桌面通常已经满足这些条件。由于凭据现在保存在本地配置文件中，不再要求 GNOME Keyring 或其他 Secret Service。
