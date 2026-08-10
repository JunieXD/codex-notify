---
title: 工作原理
description: codex-notify 如何接收 Codex 完成事件、确认中断并发送飞书卡片。
---

# 工作原理

`codex-notify` 完全运行在用户电脑上，由完成通知链、Codex Hook 和后台 watcher 三部分协作。

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

## 正常完成流程

1. Codex 完成一轮任务并触发用户级 `notify`；
2. Computer Use 如已启用，会先处理自己的事件；
3. `codex-notify` 读取完成事件和 Hook 保存的任务上下文；
4. 原有 notifier 如存在，会继续收到同一事件；
5. `codex-notify` 生成飞书卡片并直接调用飞书开放平台。

## 中断确认流程

1. watcher 增量读取最近变化的 Codex 会话记录；
2. `Stop` Hook 在缺少最终消息时提供一个候选事件；
3. 发现网络、服务或用量限制等异常后，先保存为待确认状态；
4. 等待后再次确认任务没有恢复，也没有正常完成事件；
5. 发送中断卡片，并用事件标识避免重复通知。

watcher 会保存读取位置，不会每次扫描完整会话历史。

## 为什么需要三个入口

- `notify` 是正常完成的直接信号；
- `UserPromptSubmit` Hook 补充任务内容和开始时间；
- watcher 与 `Stop` Hook 负责没有正常完成事件的异常情况。

只依赖其中一个入口，都无法同时获得完整任务信息、可靠完成通知和中断提醒。

## 本地优先的数据流

项目没有中转服务器、共享数据库或遥测服务。任务信息从本机 Codex 会话进入 `codex-notify`，然后直接发送到你配置的飞书接收者。

更完整的实现约束见[项目规格](/specification)，隐私边界见[隐私与安全](/reference/security)。
