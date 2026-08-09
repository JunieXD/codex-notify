---
name: write-release-notes
description: Write and validate concise Chinese release announcements for codex-notify from the Git differences between a target version and its previous version tag. Use before creating a version tag or GitHub Release, or when asked to draft, update, or check docs/releases/v*.md. Do not use for generic commit messages or engineering-only changelogs.
---

# 撰写更新公告

为每个版本生成一份面向用户的本地更新公告。以实际 Git 差异为准，合并同类变化，说明用户能获得什么，不照抄提交标题。

## 工作流程

1. 在仓库根目录确认目标版本。版本必须与 `Cargo.toml` 的 `package.version` 一致。
2. 确认产品变更已经提交。不要把未提交代码假定为版本内容，也不要改动无关文件。
3. 生成公告骨架和 Git 上下文：

   ```sh
   python3 scripts/release_notes.py prepare vX.Y.Z
   ```

   Windows 没有 `python3` 命令时使用 `python`。如果公告文件已经存在，运行：

   ```sh
   python3 scripts/release_notes.py context vX.Y.Z
   ```

   脚本默认选择最近的版本标签作为起点；首次发布会读取完整历史。需要指定起点时传入 `--base <ref>`。
4. 阅读脚本输出、完整 diff 和相关文档。不要只根据 `feat`、`fix` 等提交前缀分类。
5. 编辑 `docs/releases/vX.Y.Z.md`，删除占位内容和空分类。
6. 校验公告：

   ```sh
   python3 scripts/release_notes.py check vX.Y.Z
   ```

7. 发布前确认公告已提交，且版本标签指向包含该公告的提交。除非用户明确要求，否则不要自行创建标签或 Release。

## 写作规则

- 开头用一句话说明本次更新最重要的用户价值。
- 只保留有内容的分类：`新增功能`、`体验优化`、`问题修复`、`重要变化`。
- 每条只表达一个变化，优先使用“现在可以……”“不再……”等直接说法。
- 合并同一功能的多个提交；不要出现提交哈希、文件名、内部里程碑或 CI 维护细节。
- 说明必要的兼容性、配置或升级动作；破坏性变化必须放在最前面。
- 避免“若干优化”“提升体验”等空泛表述。没有用户影响的内部重构不写。
- 使用简体中文，语气友好、简洁。通常每类 2–6 条，全文不超过 12 条更新。
- 保留可直接执行的安装与升级说明。

## 发布约束

- 不以自动生成的 GitHub Notes 代替本地公告。
- 不覆盖已经写好的公告；需要重新取差异时使用 `context`。
- `check` 未通过时不得打标签。
- 更新公告、版本号和标签必须完全一致。
