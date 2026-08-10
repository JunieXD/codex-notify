---
title: 命令手册
description: codex-notify 所有面向用户的命令、参数和常见用法。
---

# 命令手册

运行下面的命令可以随时查看内置中文帮助：

```sh
codex-notify --help
codex-notify <命令> --help
```

## `init`：初始化或重新配置

```sh
codex-notify init
```

交互式配置飞书凭证、接收者、Codex 通知链、Hook 和后台监听。已有配置会先显示摘要并默认保留，不会直接覆盖。

常用选项：

| 选项 | 作用 |
| --- | --- |
| `--skip-test` | 配置完成后不发送测试通知 |
| `-y`、`--yes` | 跳过最终确认，适合明确了解变更范围的自动化环境 |
| `--app-id` | 直接提供飞书 App ID |
| `--receiver-id-type` | 指定 `email`、`open_id`、`user_id` 或 `chat_id` |
| `--receiver-id` | 直接提供对应的接收者 |

::: warning 不建议在命令中传入 App Secret
`--app-secret` 可能被终端历史记录或进程列表保存。日常使用请让交互式向导安全读取并隐藏输入。
:::

## `test`：发送测试通知

```sh
codex-notify test
```

使用当前配置发送一条飞书测试卡片。它适合确认 App ID、App Secret、权限和接收者是否都能正常工作。

## `status`：查看状态

```sh
codex-notify status
codex-notify status --json
```

显示配置文件、Codex 集成、Hook 和后台监听状态，不会显示 App Secret。`--json` 提供稳定的英文键名，方便脚本读取。

## `doctor`：检查问题

```sh
codex-notify doctor
codex-notify doctor --json
```

检查本机配置是否完整，并针对常见问题给出处理建议。遇到通知异常时，建议先运行它。

## `sync`：重新接入当前配置

```sh
codex-notify sync
```

当其他工具切换或重写 `config.toml` 后，立即把 `codex-notify` 安全接回当前通知链。后台监听也会自动完成相同检查。

## `update`：检查或安装更新

```sh
codex-notify update
```

下载最新发行包、验证 SHA-256，并安全替换当前程序。升级失败时会恢复原程序和后台服务。

常用选项：

| 选项 | 作用 |
| --- | --- |
| `--check` | 只检查是否有新版本，不执行升级 |
| `--version vX.Y.Z` | 安装指定版本 |
| `--force` | 允许重新安装当前版本或明确降级 |
| `-y`、`--yes` | 跳过升级确认 |
| `--proxy <URL>` | 使用指定的 HTTP 或 HTTPS 代理检查并下载更新 |
| `--no-proxy` | 忽略环境变量和 Windows 系统代理，直接连接更新服务器 |

Windows 会自动使用“设置 → 网络和 Internet → 代理”中已启用的手动系统代理。所有平台仍支持标准的 `HTTP_PROXY`、`HTTPS_PROXY` 和 `ALL_PROXY` 环境变量；命令行 `--proxy` 的优先级最高。

## `watch`：运行中断监听

```sh
codex-notify watch
codex-notify watch --once
```

正常情况下后台服务会自动运行 `watch`，无需手动启动。`--once` 只扫描一次，适合诊断。

## `uninstall`：移除集成

```sh
codex-notify uninstall
```

停止后台监听，移除由 `codex-notify` 管理的 Hook、凭据和状态，并恢复已记录配置原有的 notifier。它不会删除 Computer Use 或其他不属于本工具的配置。

添加 `--yes` 可以跳过卸载确认。
