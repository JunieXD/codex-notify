# 发布流程

每个版本都必须先在仓库中写好更新公告，再创建版本标签。GitHub Release 会直接使用这份公告，不会自动拼接提交记录。

## 1. 准备版本

1. 更新 `Cargo.toml` 和 `Cargo.lock` 中的版本号。
2. 提交本次产品变更，确认工作区没有混入无关修改。
3. 在 Codex 中运行：

   ```text
   使用 $write-release-notes 为 vX.Y.Z 生成更新公告。
   ```

也可以手动生成骨架和 Git 差异上下文：

```sh
python3 scripts/release_notes.py prepare vX.Y.Z
```

## 2. 校验

```sh
python3 scripts/release_notes.py check vX.Y.Z
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

确认 `docs/releases/vX.Y.Z.md` 简洁说明了用户能感知的新增功能、体验优化和问题修复。

## 3. 发布

先提交并推送版本号与更新公告，再创建标签：

```sh
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin main
git push origin vX.Y.Z
```

Release workflow 会再次校验本地公告，构建 macOS 和 Windows 安装包，生成 `SHA256SUMS`，最后发布 GitHub Release。
