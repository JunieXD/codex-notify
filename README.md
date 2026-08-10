<div align="center">

<h1><img src="assets/logo.svg" alt="codex-notify logo" width="112" height="112"><br>codex-notify</h1>

**让 Codex 完成任务或意外中断时，及时在飞书提醒你。**

[![Release](https://img.shields.io/github/v/release/JunieXD/codex-notify?display_name=tag&style=flat-square)](https://github.com/JunieXD/codex-notify/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/JunieXD/codex-notify/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/JunieXD/codex-notify/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/文档-GitHub%20Pages-5b6ee1?style=flat-square)](https://juniexd.github.io/codex-notify/)
![Platform](https://img.shields.io/badge/平台-macOS%20%7C%20Windows%20%7C%20Linux-5b6ee1?style=flat-square)
[![License](https://img.shields.io/github/license/JunieXD/codex-notify?style=flat-square)](LICENSE)

[官网与文档](https://juniexd.github.io/codex-notify/) · [快速开始](#快速开始) · [功能](#主要功能) · [更新公告](https://github.com/JunieXD/codex-notify/releases)

</div>

`codex-notify` 是一个非官方、本地优先的 Codex 通知工具。你可以让 Codex 在后台安心工作，任务结束后直接从飞书查看结果；如果任务因网络、服务或用量限制而中断，也能及时收到提醒。

## 主要功能

- **完成提醒**：展示任务标题、耗时、原始任务和完整结果。
- **中断提醒**：识别网络断开、服务异常、用量限制等终止情况。
- **减少误报**：异常出现后会等待并确认任务没有自动恢复，再发送提醒。
- **登录自启**：安装低资源占用的后台 watcher，无需管理员权限。
- **兼容 Computer Use**：保持 Computer Use 在通知链最外层，避免重复执行。
- **适配多套配置**：切换 `config.toml` 后自动补回集成，并保留各自原有 notifier。
- **统一配置**：消息平台设置和凭据统一保存在 `~/.codex-notify/config.toml`，便于备份和迁移。
- **随时撤销**：保留原有 Codex notifier，卸载时恢复原配置。

## 支持平台

支持 macOS Apple Silicon / Intel、Windows x64，以及 Linux ARM64 / x64。安装器会自动选择正确版本，安装和运行都不需要管理员权限。

完整环境要求见[支持平台](docs/reference/platforms.md)。

## 快速开始

### 1. 安装

macOS 与 Linux：

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

安装脚本会自动下载当前平台的最新发行包并校验 SHA-256。它只安装可执行文件，不会修改 Codex 或飞书配置。

### 2. 准备飞书应用

创建一个飞书企业自建应用，启用机器人和发送消息权限，然后准备 App ID、App Secret 与接收者邮箱。

第一次使用飞书机器人，请跟随[飞书应用配置教程](docs/guide/feishu-setup.md)逐步操作。教程会解释专业词汇和每个配置入口。

### 3. 初始化

```sh
codex-notify init
```

中文向导会说明每项信息的获取位置。App Secret 输入时不会显示内容，接收方式可用方向键选择；填错时可以直接重新输入，无需重新运行命令。确认前不会修改任何文件，确认后会先备份现有配置。

### 4. 检查是否可用

```sh
codex-notify test
codex-notify doctor
```

按照初始化结束时的提示信任 `UserPromptSubmit` 和 `Stop` Hook。收到测试卡片且 `doctor` 没有报错，就可以正常使用了。

更完整的安装说明见[快速开始文档](docs/guide/getting-started.md)。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `codex-notify init` | 配置飞书并接入 Codex |
| `codex-notify test` | 发送一条测试通知 |
| `codex-notify status` | 查看安装和配置状态 |
| `codex-notify doctor` | 检查常见配置问题 |
| `codex-notify sync` | 立即同步当前 `config.toml` 的通知链 |
| `codex-notify update` | 安全升级到最新版本 |
| `codex-notify uninstall` | 移除集成并恢复原配置 |

完整参数和使用场景见[命令手册](docs/guide/commands.md)。

## 文档

| 内容 | 适合什么时候看 |
| --- | --- |
| [快速开始](docs/guide/getting-started.md) | 从安装到收到第一条通知 |
| [飞书应用配置](docs/guide/feishu-setup.md) | 第一次创建飞书机器人 |
| [命令手册](docs/guide/commands.md) | 查询初始化、检查、升级和卸载命令 |
| [Codex 配置与共存](docs/guide/configuration.md) | 已有 notifier、Computer Use 或多套配置 |
| [排查常见问题](docs/guide/troubleshooting.md) | 通知、Hook 或后台监听不正常 |
| [隐私与安全](docs/reference/security.md) | 了解读取的数据、密钥与备份方式 |

也可以访问完整的[在线文档](https://juniexd.github.io/codex-notify/)，使用侧边栏和全文搜索浏览。

## 升级

日常升级运行：

```sh
codex-notify update
```

也可以重新运行安装命令。升级会验证 SHA-256，并在失败时自动恢复旧程序。详见[升级与卸载](docs/guide/update-uninstall.md)。

## 隐私与安全

- 没有云端中转服务、遥测或行为分析。
- App Secret 会明文写入仅限当前用户访问的 `~/.codex-notify/config.toml`，但不会写入日志或状态输出。
- 任务内容和结果只发送给你配置的飞书接收者。
- 修改 Codex 配置前会在本地创建带时间戳的备份。

详细说明见[隐私与安全](docs/reference/security.md)。开发者可以继续阅读[项目规格](docs/specification.md)和[发布流程](docs/releasing.md)。

## 许可证

[MIT](LICENSE) © JunieXD
