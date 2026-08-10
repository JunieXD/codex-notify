#!/usr/bin/env python3
"""Prepare and validate user-facing release announcements from Git history."""

from __future__ import annotations

import argparse
import datetime as dt
import re
import subprocess
import sys
from pathlib import Path


SEMVER_RE = re.compile(r"^v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$")
PLACEHOLDER_RE = re.compile(r"\b(?:TODO|TBD)\b|待补充|请填写|一句话说明")
DECORATIVE_EMOJI_RE = re.compile(
    r"[\u2600-\u27BF\U0001F1E6-\U0001F1FF\U0001F300-\U0001FAFF]"
)
UPDATE_HEADINGS = (
    "## 新增功能",
    "## 体验优化",
    "## 问题修复",
    "## 重要变化",
)
INSTALL_HEADING = "## 安装与升级"


class ReleaseNotesError(RuntimeError):
    """A concise, actionable release-notes error."""


def run_git(*args: str, cwd: Path | None = None, input_text: str | None = None) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=cwd,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "Git 命令执行失败"
        raise ReleaseNotesError(detail)
    return result.stdout.strip()


def repository_root() -> Path:
    return Path(run_git("rev-parse", "--show-toplevel")).resolve()


def normalize_version(value: str) -> str:
    match = SEMVER_RE.fullmatch(value.strip())
    if not match:
        raise ReleaseNotesError(f"版本号格式无效：{value}；示例：v0.2.0")
    return f"v{match.group(1)}"


def package_version(root: Path) -> str:
    in_package = False
    for raw_line in (root / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if line == "[package]":
            in_package = True
            continue
        if in_package and line.startswith("["):
            break
        if in_package:
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    raise ReleaseNotesError("无法从 Cargo.toml 读取 package.version")


def previous_version_tag(root: Path, version: str, head: str) -> str | None:
    output = run_git("tag", "--merged", head, "--sort=-version:refname", cwd=root)
    for tag in output.splitlines():
        tag = tag.strip()
        if tag and tag != version and SEMVER_RE.fullmatch(tag):
            return tag
    return None


def empty_tree_hash(root: Path) -> str:
    return run_git("hash-object", "-t", "tree", "--stdin", cwd=root, input_text="")


def build_context(root: Path, version: str, base: str | None, head: str) -> str:
    resolved_base = base or previous_version_tag(root, version, head)
    commit_range = f"{resolved_base}..{head}" if resolved_base else head
    diff_base = resolved_base or empty_tree_hash(root)
    commits = run_git(
        "log",
        "--no-merges",
        "--format=- %h %s",
        commit_range,
        cwd=root,
    )
    changed_files = run_git("diff", "--name-status", diff_base, head, cwd=root)
    diff_stat = run_git("diff", "--stat", diff_base, head, cwd=root)
    base_label = resolved_base or "首次发布（完整历史）"
    return "\n".join(
        (
            f"# {version} Git 差异上下文",
            "",
            f"- 对比起点：{base_label}",
            f"- 对比终点：{head}",
            "",
            "## 提交记录",
            "",
            commits or "- 无提交记录",
            "",
            "## 变更文件",
            "",
            "```text",
            changed_files or "无文件变化",
            "```",
            "",
            "## 变更统计",
            "",
            "```text",
            diff_stat or "无文件变化",
            "```",
        )
    )


def announcement_template(version: str) -> str:
    today = dt.date.today().isoformat()
    return f"""# {version} 更新公告

> 发布日期：{today}

一句话说明这次更新能为用户带来什么。

## 新增功能

- TODO：说明用户现在可以完成什么。

## 体验优化

- TODO：说明哪些操作变得更简单或更可靠。

## 问题修复

- TODO：说明修复了哪些会影响用户的问题。

## 安装与升级

已有用户可直接升级：

```sh
codex-notify update
```

首次安装可使用以下命令。

### macOS 与 Linux

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

安装完成后运行 `codex-notify init` 开始配置。
"""


def notes_path(root: Path, version: str) -> Path:
    return root / "docs" / "releases" / f"{version}.md"


def prepare_notes(
    root: Path,
    version: str,
    base: str | None,
    head: str,
    force: bool,
) -> None:
    path = notes_path(root, version)
    if path.exists() and not force:
        raise ReleaseNotesError(
            f"{path.relative_to(root)} 已存在；请先审阅，或使用 context 重新查看差异"
        )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(announcement_template(version), encoding="utf-8")
    print(f"已生成：{path.relative_to(root)}")
    print()
    print(build_context(root, version, base, head))


def section_body(markdown: str, heading: str) -> str | None:
    marker = f"{heading}\n"
    start = markdown.find(marker)
    if start < 0:
        return None
    start += len(marker)
    end = markdown.find("\n## ", start)
    return markdown[start:] if end < 0 else markdown[start:end]


def validate_notes(root: Path, version: str) -> Path:
    cargo_version = package_version(root)
    if version.removeprefix("v") != cargo_version:
        raise ReleaseNotesError(
            f"公告版本 {version} 与 Cargo.toml 版本 {cargo_version} 不一致"
        )

    path = notes_path(root, version)
    if not path.is_file():
        raise ReleaseNotesError(f"缺少更新公告：{path.relative_to(root)}")
    markdown = path.read_text(encoding="utf-8")
    errors: list[str] = []

    if not markdown.startswith(f"# {version} 更新公告\n"):
        errors.append(f"首行必须是：# {version} 更新公告")
    if not re.search(r"^> 发布日期：\d{4}-\d{2}-\d{2}$", markdown, re.MULTILINE):
        errors.append("缺少格式正确的发布日期")
    if PLACEHOLDER_RE.search(markdown):
        errors.append("仍有 TODO 或占位文案")
    if DECORATIVE_EMOJI_RE.search(markdown):
        errors.append("请移除装饰性 emoji，分类标题和正文应使用纯文本")

    structure_markdown = DECORATIVE_EMOJI_RE.sub("", markdown)
    structure_markdown = structure_markdown.replace("\ufe0f", "").replace("\u200d", "")
    structure_markdown = re.sub(r"^##\s+", "## ", structure_markdown, flags=re.MULTILINE)

    present_updates = 0
    for heading in UPDATE_HEADINGS:
        body = section_body(structure_markdown, heading)
        if body is None:
            continue
        present_updates += 1
        if not re.search(r"^- \S", body, re.MULTILINE):
            errors.append(f"{heading} 没有有效条目；无内容时请删除整个分类")
    if present_updates == 0:
        errors.append("至少保留一个更新分类")

    install_body = section_body(structure_markdown, INSTALL_HEADING)
    if install_body is None or "codex-notify" not in install_body:
        errors.append("缺少可执行的安装或升级说明")
    if not markdown.endswith("\n"):
        errors.append("文件末尾缺少换行")

    if errors:
        formatted = "\n".join(f"- {item}" for item in errors)
        raise ReleaseNotesError(f"更新公告校验失败：\n{formatted}")
    return path


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="根据 Git 版本差异准备并校验 codex-notify 更新公告"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    for command in ("prepare", "context"):
        subparser = subparsers.add_parser(command)
        subparser.add_argument("version", help="目标版本，例如 v0.2.0")
        subparser.add_argument("--base", help="对比起点；默认使用最近的版本标签")
        subparser.add_argument("--head", default="HEAD", help="对比终点，默认 HEAD")
        if command == "prepare":
            subparser.add_argument("--force", action="store_true", help="覆盖已有公告骨架")

    check = subparsers.add_parser("check")
    check.add_argument("version", help="要校验的版本，例如 v0.2.0")
    subparsers.add_parser("check-current")
    return parser


def main() -> int:
    try:
        args = build_parser().parse_args()
        root = repository_root()
        if args.command == "check-current":
            version = normalize_version(package_version(root))
        else:
            version = normalize_version(args.version)

        if args.command == "prepare":
            prepare_notes(root, version, args.base, args.head, args.force)
        elif args.command == "context":
            print(build_context(root, version, args.base, args.head))
        else:
            path = validate_notes(root, version)
            print(f"更新公告校验通过：{path.relative_to(root)}")
        return 0
    except ReleaseNotesError as error:
        print(f"错误：{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
