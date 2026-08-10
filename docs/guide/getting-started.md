---
title: 快速开始
description: 安装 codex-notify，配置飞书并收到第一条 Codex 通知。
---

# 快速开始

按照下面的顺序操作，通常几分钟就能收到第一条测试通知。

## 使用前准备

- 一台 macOS、Windows 或 Linux 电脑；
- 已安装并使用 Codex CLI 或 ChatGPT App 中的 Codex；
- 一个可以创建企业自建应用的飞书账号。

安装后的程序是独立可执行文件，不需要 Rust、Python 或 Node.js，也不需要管理员权限。

## 1. 安装

### macOS 与 Linux

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

安装脚本会识别当前平台，下载最新发行包并校验 SHA-256。它只安装 `codex-notify` 程序，不会立即修改 Codex 或飞书配置。

安装完成后检查版本：

```sh
codex-notify --version
```

如果终端提示找不到命令，请按照安装脚本最后显示的说明把安装目录加入 `PATH`，然后重新打开终端。

::: info Linux 桌面环境
Linux 需要 systemd 用户会话和 Secret Service。Ubuntu 桌面通常已经包含 GNOME Keyring，无需额外配置。
:::

## 2. 准备飞书应用

你需要创建一个飞书企业自建应用，启用机器人能力和发送消息权限，然后取得 App ID 与 App Secret。

如果没有使用过飞书开放平台，请直接跟随[飞书应用配置教程](/guide/feishu-setup)。教程会解释每一个术语和操作入口。

## 3. 运行初始化

```sh
codex-notify init
```

中文向导会依次询问：

1. 飞书 App ID；
2. 飞书 App Secret；
3. 接收方式，首次使用建议选择邮箱；
4. 接收者邮箱或对应 ID。

App Secret 输入时不会显示字符，这是正常现象。确认前不会修改任何文件；确认后会先备份现有 Codex 配置，再写入通知链和 Hook。

如果已经配置过，再次运行 `init` 会显示不含密钥的配置摘要，并默认保留当前配置。只有主动选择“重新配置”才会替换飞书设置。

## 4. 信任 Codex Hook

初始化会添加 `UserPromptSubmit` 和 `Stop` 两个 Hook。它们分别用于记录任务开始信息，以及在任务意外结束时提供兜底判断。

### ChatGPT App（原 Codex App）

打开“设置” → “钩子”，在“用户”区域把 `UserPromptSubmit` 和 `Stop` 分别设为“信任”。

### Codex CLI

在 Codex 中运行：

```text
/hooks
```

然后信任这两个 Hook。

## 5. 发送测试通知

```sh
codex-notify test
codex-notify doctor
```

收到飞书测试卡片，并且 `doctor` 没有报告错误，就可以正常使用了。

如果没有收到消息，请查看[排查常见问题](/guide/troubleshooting)。

## 下一步

- [了解每个命令的用途](/guide/commands)
- [查看通知会在什么时候发送](/guide/notifications)
- [了解它如何与现有 notifier 和 Computer Use 共存](/guide/configuration)
