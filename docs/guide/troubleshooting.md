---
title: 排查常见问题
description: 按顺序检查飞书权限、Codex Hook、后台监听和配置切换问题。
---

# 排查常见问题

遇到问题时，先运行：

```sh
codex-notify status
codex-notify doctor
```

`status` 显示当前状态和实际路径，`doctor` 会检查常见配置错误并给出下一步建议。两者都不会显示 App Secret。

## 没有收到测试通知

先再次测试：

```sh
codex-notify test
```

然后依次确认：

1. 飞书应用已经发布；
2. 机器人能力已经启用；
3. 发送消息权限已经审批并生效；
4. 接收者属于同一个飞书组织，并在应用可用范围内；
5. App ID 和 App Secret 来自同一个应用；
6. 使用群聊 ID 时，机器人已经加入目标群。

详细步骤见[飞书应用配置教程](/guide/feishu-setup)。

## 测试通知正常，但 Codex 完成后没有消息

通常是后台 watcher 没有运行、Hook 尚未信任，或当前 `config.toml` 没有接入通知链。

1. 在 ChatGPT App 的“设置” → “钩子”中信任 `UserPromptSubmit` 和 `Stop`；Codex CLI 用户运行 `/hooks`。
2. 运行 `codex-notify doctor` 查看通知链。
3. 如果刚切换过 Codex 配置，运行：

   ```sh
   codex-notify sync
   ```

升级 `codex-notify` 后不需要重启 ChatGPT。已经打开的旧任务会由 watcher 从本地任务记录补获；如果 `doctor` 显示后台监听异常，请先修复 watcher。

## 切换配置后通知失效

后台 watcher 会自动检测配置切换，但也可以立即同步：

```sh
codex-notify sync
```

如果同步提示 Computer Use 包装无法识别，请不要手动删除未知参数。保留 `doctor` 输出并到 GitHub 提交问题。

## 收到两条飞书通知

`codex-notify` 会保留原有 notifier。如果旧 notifier 也发送飞书消息，两个工具会各自发送一条。

先确认 `codex-notify test` 和正常任务通知都可用，再停用旧 notifier 的飞书发送功能。不要直接删除整条 `notify` 配置，否则可能同时影响 Computer Use。

## 没有收到中断通知

手动扫描一次：

```sh
codex-notify watch --once
```

再运行 `codex-notify doctor` 检查后台服务。请注意，中断检测依赖 Codex 本地记录；断电、系统崩溃或强制结束进程时，可能没有足够信息发送提醒。

## App Secret 无法读取

系统凭据可能被删除、锁定或迁移失败。重新运行：

```sh
codex-notify init
```

选择“重新配置”，再次输入同一个应用的 App Secret。写入前会备份现有 codex-notify 配置。

macOS 从旧版本首次升级时可能弹出钥匙串授权窗口，请选择“始终允许”。授权对象应为系统自带的 `/usr/bin/security`；迁移完成后，后续升级不需要重复授权。如果升级时拒绝授权，工具会恢复旧版本和原有 watcher，可以重新运行 `codex-notify update` 再试。

## 获取便于反馈的信息

```sh
codex-notify status --json
codex-notify doctor --json
```

JSON 输出使用稳定键名，适合复制到问题报告中。提交前仍建议检查路径、邮箱等信息是否需要打码；不要提供 App Secret、访问令牌或系统凭据内容。

仍无法解决时，可以在 [GitHub Issues](https://github.com/JunieXD/codex-notify/issues) 中说明操作系统、Codex 使用方式、复现步骤和已脱敏的 `doctor` 输出。
