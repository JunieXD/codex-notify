---
title: Codex 配置与共存
description: 了解 notify、Hook、Computer Use 和多套 config.toml 如何与 codex-notify 共存。
---

# Codex 配置与共存

`codex-notify init` 会修改 Codex 用户配置，但不会简单覆盖原内容。它会先解析现有配置、创建备份，再把自己接入正确位置。

## 与已有 notifier 共存

Codex 的 `notify` 配置只能指向一条外部命令。如果你已经使用其他通知程序，`codex-notify` 会保存并继续调用它。

通知顺序是：

```text
Codex → codex-notify → 原有 notifier（如果存在）
```

每个程序独立执行。原 notifier 失败时，不会阻止 `codex-notify` 尝试发送飞书消息；反过来也一样。

## 与 Computer Use 共存

Computer Use 会把自己放在 Codex 通知链最外层，并通过内部的 `--previous-notify` 参数保留原命令。`codex-notify` 能识别已经测试过的 Computer Use 包装，并维持下面的顺序：

```text
Codex → Computer Use → codex-notify → 原有 notifier（如果存在）
```

它不会反向调用 Computer Use，因此不会形成循环或重复执行。无法识别或已经损坏的 Computer Use 包装不会被强行覆盖，`doctor` 会提示你人工检查。

::: info 关于 `--previous-notify`
这是 Computer Use 当前使用的内部配置方式，不是 Codex 已公开保证稳定的接口。`codex-notify` 只处理经过测试的结构，遇到未知形式会优先保护现有配置。
:::

## Hook 会做什么

初始化会添加两个用户级 Hook：

| Hook | 用途 |
| --- | --- |
| `UserPromptSubmit` | 记录任务内容和开始时间，为通知补全上下文 |
| `Stop` | 在没有正常完成消息时留下候选中断信息，由 watcher 再次确认 |

已有的其他 Hook 会原样保留。ChatGPT App 或 Codex CLI 首次发现新 Hook 时，需要你手动信任。

## 多套 `config.toml`

一些配置管理工具会在同一个 `CODEX_HOME` 中切换不同的 `config.toml`。后台 watcher 会检测当前配置，并在需要时重新接入通知链。

每套配置原有的 notifier 都保存在自己的托管命令中，不会与其他配置混用。需要立即检查时运行：

```sh
codex-notify sync
```

符号链接也受支持。写入配置时会更新链接指向的目标文件，不会把符号链接本身替换掉。

## 重复运行初始化

```sh
codex-notify init
```

检测到已有配置时，CLI 会显示 App ID、接收方式、接收者和配置路径，但不会显示 App Secret。默认操作是保留当前配置并退出。

只有明确选择“重新配置”后，才会替换飞书设置和 App Secret。Codex 原有 notifier、其他 Hook 和相关备份仍会保留。

## 配置与备份位置

运行下面的命令可以查看当前机器上的实际路径：

```sh
codex-notify status
```

应用配置、运行状态、日志和备份保存在当前用户的应用数据目录中；App Secret 单独保存在系统凭据库。路径会随操作系统变化，因此建议以 `status` 输出为准。
