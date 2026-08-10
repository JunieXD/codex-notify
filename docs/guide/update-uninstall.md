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

升级会自动重启 watcher，不会重启 ChatGPT。已经打开的旧任务即使没有重新加载 `notify`，升级后的 watcher 也会从本地任务记录补获后续完成事件，因此通常不需要手动退出 ChatGPT。首次启用 Hook 后仍需按提示完成信任。

::: tip macOS 首次迁移钥匙串权限
从旧版本升级时，macOS 可能弹出一次钥匙串授权窗口。请选择“始终允许”；codex-notify 会把凭据访问迁移给系统自带的 `/usr/bin/security`，后续升级不会再因程序版本变化重复询问。迁移会在 watcher 重启前完成，失败时仍会恢复旧程序和后台服务。
:::

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
