# codex-notify

`codex-notify` is an unofficial, local-first notification tool for Codex.
Its first delivery channel is Feishu. The project is designed to add other
notification providers without changing its Codex integration.

## Status

The repository currently contains the Rust foundation, release automation, and
the complete product specification. The operational commands are intentionally
not implemented yet.

## Intended experience

```text
codex-notify init
codex-notify test
codex-notify doctor
```

`init` will eventually configure a user's existing Codex notification command
without overwriting it, securely collect Feishu configuration, install a local
error watcher, and verify the result with a test card.

## Install

After the first GitHub Release is published, install the latest macOS build:

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
```

On Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
```

Both scripts download the matching release artifact, verify it against the
release `SHA256SUMS`, and install only the binary. They do not configure Codex
or Feishu; run `codex-notify init` after installation once that command is
implemented. The scripts use a user-writable directory and print the exact
PATH update needed when the directory is not already on `PATH`.

To install a specific release, download the script and pass `CODEX_NOTIFY_VERSION`
on macOS or the `-Version` argument on Windows. The version must include its
`v` prefix, for example `v0.1.0`.

## Goals

- macOS and Windows support from one Rust codebase.
- Feishu first, with provider adapters for future channels.
- Completion and terminal-error notifications with a conversation title,
  status prefix, duration, task, and Markdown details.
- No hosted relay service and no telemetry by default.
- Secrets stored in the operating system credential store, not in plaintext
  configuration files.

## Documentation

See [the specification](docs/specification.md) for the architecture, CLI
contract, platform integration, Feishu setup, security model, and release
requirements.

## Development

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
```

## License

MIT. See [LICENSE](LICENSE).
