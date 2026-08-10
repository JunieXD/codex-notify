---
title: 隐私与安全
description: codex-notify 读取哪些本地信息、如何保存密钥，以及用户可以如何撤销变更。
---

# 隐私与安全

`codex-notify` 采用本地优先设计：没有项目运营的中转服务、账号系统、遥测或行为分析。

## 会处理哪些数据

为了生成通知，工具可能读取：

- Codex 提供的完成事件；
- 当前任务内容和最终回复；
- Codex 会话标题、时间和工作目录；
- 本地会话记录中的完成状态和异常信息；
- 当前用户的 `config.toml` 和 `hooks.json`。

这些内容只在本机处理，并直接发送给你配置的飞书接收者。待发送的完成事件会暂存在 codex-notify 的本地状态目录，发送成功后即从队列移除；队列和去重记录都有数量上限。

## App Secret 如何保存

App Secret 不会明文写入配置文件或日志，而是保存在操作系统提供的凭据库：

| 平台 | 凭据存储 |
| --- | --- |
| macOS | Keychain（钥匙串），通过系统自带的 `/usr/bin/security` 访问 |
| Windows | Credential Manager（凭据管理器） |
| Linux | Secret Service，例如 GNOME Keyring |

App ID、接收方式和接收者属于非密钥配置，会保存在 codex-notify 的应用数据目录中。

macOS 旧版本曾让钥匙串直接信任某一版 codex-notify 的程序哈希，替换程序后可能再次询问权限。升级流程现在会在前台完成一次迁移，让 Apple 签名且路径稳定的系统凭据工具读取该条目；写入仍通过原生 Keychain API 完成。App Secret 不会出现在命令行、进程列表或日志中。

## 配置保护

- 修改 Codex 配置前会创建带时间戳的备份；
- TOML 和 JSON 会经过解析后修改，不使用容易破坏格式的文本替换；
- 现有 notifier 和无关 Hook 会保留；
- 未知或损坏的 Computer Use 包装不会被强行覆盖；
- 配置写入采用原子替换，并支持符号链接目标。

## 网络连接

正常使用时会连接：

- 飞书开放平台：获取应用访问令牌并发送消息；
- GitHub：仅在检查更新、升级或运行安装脚本时访问发行版。

项目不会把任务内容发送到其他第三方服务。

## 用户可以随时撤销

```sh
codex-notify uninstall
```

卸载会停止后台服务、移除自身 Hook 和凭据，并恢复已记录配置原有的 notifier。详细行为见[升级与卸载](/guide/update-uninstall)。

## 使用建议

- 不要把 App Secret 写在 Shell 脚本、截图或问题报告中；
- 飞书应用只开通发送消息所需的最小权限；
- 使用群聊接收通知前，确认群成员都可以查看任务内容；
- 分享 `status --json` 或 `doctor --json` 前，对邮箱和本地路径进行必要脱敏。
