<div align="center">

<h1><img src="assets/logo.svg" alt="codex-notify logo" width="112" height="112"><br>codex-notify</h1>

**让 Codex 完成任务或意外中断时，及时在飞书提醒你。**

[![Release](https://img.shields.io/github/v/release/JunieXD/codex-notify?display_name=tag&style=flat-square)](https://github.com/JunieXD/codex-notify/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/JunieXD/codex-notify/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/JunieXD/codex-notify/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/平台-macOS%20%7C%20Windows-5b6ee1?style=flat-square)
[![License](https://img.shields.io/github/license/JunieXD/codex-notify?style=flat-square)](LICENSE)

[快速开始](#快速开始) · [功能](#主要功能) · [常用命令](#常用命令) · [隐私安全](#隐私与安全) · [更新公告](https://github.com/JunieXD/codex-notify/releases)

</div>

`codex-notify` 是一个非官方、本地优先的 Codex 通知工具。你可以让 Codex 在后台安心工作，任务结束后直接从飞书查看结果；如果任务因网络、服务或用量限制而中断，也能及时收到提醒。

## 主要功能

- **完成提醒**：展示任务标题、耗时、原始任务和完整结果。
- **中断提醒**：识别网络断开、服务异常、用量限制等终止情况。
- **减少误报**：异常出现后会等待并确认任务没有自动恢复，再发送提醒。
- **登录自启**：安装低资源占用的后台 watcher，无需管理员权限。
- **兼容 Computer Use**：保持 Computer Use 在通知链最外层，避免重复执行。
- **适配多套配置**：切换 `config.toml` 后自动补回集成，并保留各自原有 notifier。
- **安全存储**：App Secret 仅保存在 macOS 钥匙串或 Windows 凭据管理器中。
- **随时撤销**：保留原有 Codex notifier，卸载时恢复原配置。

## 支持平台

| 平台 | 安装包 | 后台启动 |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | LaunchAgent |
| macOS Intel | `x86_64-apple-darwin` | LaunchAgent |
| Windows x64 | `x86_64-pc-windows-msvc` | 当前用户登录项 |

Linux 暂未提供发行包。

## 快速开始

### 1. 安装

macOS：

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

安装脚本会自动下载当前平台的最新发行包并校验 SHA-256。它只安装可执行文件，不会修改 Codex 或飞书配置。

### 2. 准备飞书应用

1. 创建一个飞书企业自建应用。
2. 开通机器人能力和发送消息权限，然后发布应用。
3. 与机器人建立私聊，准备好 App ID、App Secret 和接收者 ID。

支持 `open_id`、`user_id`、邮箱和群聊 `chat_id`。私聊场景建议使用 `open_id`。

### 3. 初始化

```sh
codex-notify init
```

根据提示填写飞书信息并确认修改。初始化完成后，在 Codex 中运行 `/hooks`，信任新增的 `UserPromptSubmit` 和 `Stop` Hook。

### 4. 检查是否可用

```sh
codex-notify test
codex-notify doctor
```

收到测试卡片且 `doctor` 没有报错，就可以正常使用了。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `codex-notify init` | 配置飞书并接入 Codex |
| `codex-notify test` | 发送一条测试通知 |
| `codex-notify status` | 查看安装和配置状态 |
| `codex-notify doctor` | 检查常见配置问题 |
| `codex-notify sync` | 立即同步当前 `config.toml` 的通知链 |
| `codex-notify watch --once` | 手动扫描一次中断事件 |
| `codex-notify uninstall` | 移除集成并恢复原配置 |

`status` 和 `doctor` 均支持 `--json`，便于脚本读取。

## 工作方式

```mermaid
flowchart LR
    A["Codex 任务"] -->|"正常完成"| B["notify 通知链"]
    B --> C["Computer Use（可选）"]
    C --> D["codex-notify"]
    A -->|"任务上下文"| E["Codex Hooks"]
    E --> D
    A -->|"异常终止"| F["本地 watcher"]
    F -->|"确认未恢复"| D
    D --> G["飞书卡片"]
```

后台 watcher 只增量读取最近变动的本地会话记录，并保存读取位置，不会反复扫描完整历史。

## 中断提醒的边界

中断检测依赖 Codex 写入本地会话记录，因此属于尽力而为。断电、系统崩溃或强制结束进程时，如果 Codex 还没来得及写入终止信息，就无法发送提醒。

## 与现有 notifier 共存

初始化会保留并继续调用已有的 Codex `notify` 命令。检测到 Computer Use 时，CLI 会保持它在最外层，再把 `codex-notify` 接入其 `--previous-notify`；`codex-notify` 不会反向调用 Computer Use，因此不会形成重复链。

如果旧 notifier 也向飞书发送消息，仍可能收到两条飞书提醒。确认 `codex-notify` 工作正常后，再停用旧工具的飞书发送功能。

## 多套 Codex 配置

如果你使用配置管理工具切换同一 `CODEX_HOME` 下的 `config.toml`，后台 watcher 会检测当前配置并自动补回通知链。每套配置原有的 notifier 会保存在它自己的托管命令中，不会与其他配置混用。

安装时会记住当前应用目录和 `CODEX_HOME`，macOS 或 Windows 重新登录后仍会监控同一套配置。

需要立即同步时运行：

```sh
codex-notify sync
```

CLI 支持配置文件符号链接，写入时会保留链接本身。无法识别或损坏的 Computer Use 包装不会被覆盖，`doctor` 会提示人工检查。

## 升级与卸载

升级时重新运行对应平台的安装命令即可。

```sh
codex-notify uninstall
```

卸载只移除 `codex-notify` 管理的 Hook、后台启动项、凭据和状态，并恢复已记录配置文件原有的 notifier。Computer Use 会继续保留在通知链最外层。

## 隐私与安全

- 没有云端中转服务、遥测或行为分析。
- App Secret 不会写入配置文件或日志。
- 任务内容和结果只发送给你配置的飞书接收者。
- 修改 Codex 配置前会在本地创建带时间戳的备份。

## 开发与发布

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

架构细节见 [项目规格](docs/specification.md)，维护者发布流程见 [发布指南](docs/releasing.md)。

## 许可证

[MIT](LICENSE) © JunieXD
