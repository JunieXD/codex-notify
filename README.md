# codex-notify

codex-notify is an unofficial, local-first notification tool for Codex. M1
sends Feishu private-message cards when a Codex turn completes. It is designed
to add other destinations later without changing the Codex integration.

## M1 Status

The completion-notification path is implemented:

- Feishu tenant-token authentication and interactive Card JSON 2.0 delivery.
- A mobile-visible outer title with a success emoji and the Codex conversation
  title.
- A collapsed card body containing the original task and full Markdown result.
- Elapsed time formatted as hours, minutes, and seconds.
- UserPromptSubmit state capture for the task, title, and start time.
- Reversible preservation of an existing Codex notify command.
- App Secret storage in macOS Keychain or Windows Credential Manager.

The M2 transcript watcher for terminal failures is not implemented yet. The
watch command intentionally reports that status instead of silently doing
nothing.

## Install

After the first GitHub Release is published, install the latest macOS build:

~~~sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.sh | sh
~~~

On Windows PowerShell:

~~~powershell
irm https://raw.githubusercontent.com/JunieXD/codex-notify/main/scripts/install.ps1 | iex
~~~

The scripts download the matching release artifact, verify SHA256SUMS, and
install only the binary into a user-writable directory. They do not modify
Codex, Feishu, shell startup files, or system-wide directories.

## Configure

Create an internal Feishu custom app with Bot capability, grant it permission to send
messages, publish it to your tenant, then open a private chat with the bot.
Use the receiver identifier appropriate to the Feishu app configuration.
For a private chat, open_id is the recommended receiver type.

Run:

~~~sh
codex-notify init
~~~

The command prompts for App ID, App Secret, receiver ID type, and receiver ID.
It shows the exact Codex files that will change and requires confirmation.
Then trust the newly added UserPromptSubmit Hook from the Codex slash command
/hooks.

Verify delivery and configuration:

~~~sh
codex-notify test
codex-notify status
codex-notify doctor
~~~

Use the JSON form for automation:

~~~sh
codex-notify status --json
codex-notify doctor --json
~~~

Remove only codex-notify-managed integration and restore the saved previous
notifier:

~~~sh
codex-notify uninstall
~~~

## Existing Notifiers

Initialization never overwrites an existing Codex notify command. It saves it,
installs a dispatcher, invokes the previous command with the original event,
then sends the codex-notify card independently.

If the previous notifier already sends Feishu messages, both tools will notify
and duplicate messages are expected. Initialization prints a warning for common
Feishu notifier names; migrate or remove the old Feishu notifier only after
confirming codex-notify works.

## Privacy and Safety

- No hosted relay, telemetry, or analytics.
- App Secret is not written to configuration files or logs.
- Task text and final output are sent only to the selected Feishu receiver.
- Before changing a user-managed Codex file, initialization writes a timestamped
  backup under the codex-notify application-data directory.
- Uninstall restores the old notifier only when the active command is
  demonstrably owned by codex-notify.

## Development

~~~sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run -- --help
~~~

See docs/specification.md for the complete architecture and release plan.

## License

MIT. See LICENSE.
