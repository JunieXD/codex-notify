---
title: 升级与卸载
description: 安全升级 codex-notify，安装指定版本，或恢复原有 Codex 配置。
---

# 升级与卸载

## 升级到最新版本

```sh
codex-notify update
```

升级会：

1. 查询最新 GitHub Release；
2. 下载当前平台的发行包和 `SHA256SUMS`；
3. 验证文件没有损坏或被替换；
4. 停止后台 watcher；
5. 备份并替换程序；
6. 刷新已有 Codex 集成，再重新启动 watcher。

飞书凭据、通知链和已记录的多套 Codex 配置都会保留。升级失败时会自动恢复旧程序和后台服务。

只检查是否有更新：

```sh
codex-notify update --check
```

## 重新运行安装脚本

已有安装时，安装脚本会自动调用同一套安全升级流程。

::: code-group

```sh [macOS / Linux]
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

```powershell [Windows PowerShell]
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

:::

## 安装指定版本

```sh
codex-notify update --version v0.4.0
```

默认不允许降级。只有明确理解影响时，才使用 `--force` 安装旧版本：

```sh
codex-notify update --version v0.4.0 --force
```

## 卸载

```sh
codex-notify uninstall
```

卸载会移除：

- 由 `codex-notify` 添加的 Hook；
- 后台启动项；
- 飞书凭据和本地运行状态；
- `codex-notify` 管理的通知链。

它会恢复已记录配置原有的 notifier，并保留 Computer Use 和其他用户配置。确认前会显示变更摘要；添加 `--yes` 可以跳过确认。

::: warning 程序文件可能仍然保留
卸载命令负责移除集成和本地数据。若还要删除可执行文件，请根据 `codex-notify status` 或终端中的安装位置手动删除。
:::
