---
layout: home
title: codex-notify
titleTemplate: false

hero:
  name: codex-notify
  text: Codex 完成了，飞书告诉你
  tagline: 一个本地优先、跨平台的命令行工具。任务正常完成或意外中断时，及时把结果发送到飞书。
  image:
    src: /logo.svg
    alt: codex-notify
  actions:
    - theme: brand
      text: 5 分钟开始使用
      link: /guide/getting-started
    - theme: alt
      text: 配置飞书应用
      link: /guide/feishu-setup
    - theme: alt
      text: 查看 GitHub
      link: https://github.com/JunieXD/codex-notify

features:
  - title: 完成提醒
    details: 在手机上直接查看任务标题、耗时、原始任务和 Codex 的完整结果。
  - title: 中断提醒
    details: 识别网络、服务和用量限制等异常，并等待确认任务没有自动恢复。
  - title: 安全共存
    details: 保留已有 notifier，兼容 Computer Use，也能适应多套 config.toml 切换。
  - title: 本地优先
    details: 没有中转服务器或遥测；App Secret 只保存在系统凭据库中。
  - title: 三端支持
    details: 提供 macOS、Windows 和 Linux 独立安装包，不需要 Rust、Python 或 Node.js。
  - title: 随时撤销
    details: 修改前自动备份，卸载时恢复原有 Codex 通知配置和其他 Hook。
---

<p class="home-intro">
让 Codex 在后台安心工作，不必反复切回窗口查看进度。安装和运行都不需要管理员权限。
</p>

## 立即安装

::: code-group

```sh [macOS / Linux]
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

```powershell [Windows PowerShell]
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

:::

安装后运行 `codex-notify init`，中文向导会带你完成飞书和 Codex 配置。第一次接触飞书开放平台，可以先阅读[飞书应用配置教程](/guide/feishu-setup)。

## 从这里继续

<div class="home-links">
  <a class="home-link" href="/codex-notify/guide/getting-started">
    <strong>快速开始</strong>
    <span>从安装到收到第一条测试通知。</span>
  </a>
  <a class="home-link" href="/codex-notify/guide/commands">
    <strong>命令手册</strong>
    <span>查看初始化、检查、升级和卸载命令。</span>
  </a>
  <a class="home-link" href="/codex-notify/guide/troubleshooting">
    <strong>排查问题</strong>
    <span>没有收到消息、Hook 未信任或后台监听异常。</span>
  </a>
</div>
