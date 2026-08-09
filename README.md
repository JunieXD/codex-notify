# codex-notify

codex-notify is an unofficial, local-first notification tool for Codex. It
sends Feishu private-message cards for completion and best-effort terminal
interruptions, and is designed to add other destinations later without changing
the Codex integration.

## Current Status

M2 is implemented:

- Feishu tenant-token authentication and interactive Card JSON 2.0 delivery.
- A mobile-visible outer title with status, send time, and the Codex
  conversation title.
- A collapsed card body containing the original task and full Markdown result.
- The local send date and time at the top of the collapsed card body.
- Elapsed time formatted as hours, minutes, and seconds.
- UserPromptSubmit state capture for the task, title, and start time.
- Known Codex desktop ambient, title, and retrieval-index turns are suppressed
  across prompt capture, completion delivery, and interruption monitoring.
- A Stop Hook fallback that records, rather than immediately sends, a missing
  final-result event.
- An incremental transcript watcher for `task_complete.error`, including
  stream disconnects, provider failures, and the exact known usage-limit
  terminal message.
- Two-stage confirmation: ordinary errors wait 30 seconds; a later
  `task_started` cancels the alert. Active Goals wait for `blocked`,
  `usage_limited`, `budget_limited`, or 10 minutes of silence.
- A per-user macOS LaunchAgent or Windows Task Scheduler task that keeps the
  low-resource watcher running after login.
- Reversible preservation of an existing Codex notify command.
- App Secret storage in macOS Keychain or Windows Credential Manager.

The watcher is deliberately best-effort. Codex does not document transcript
JSONL as a stable extension interface, and no local tool can alert after a
power loss or force quit before Codex writes a terminal record.

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
Then trust the newly added UserPromptSubmit and Stop Hooks from the Codex slash
command `/hooks`.

Verify delivery and configuration:

~~~sh
codex-notify test
codex-notify status
codex-notify doctor
codex-notify watch --once
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
then sends the codex-notify card independently. It also installs only its own
UserPromptSubmit and Stop Hook handlers, leaving unrelated handlers in place.

If the previous notifier already sends Feishu messages, both tools will notify
and duplicate messages are expected. Initialization prints a warning for common
Feishu notifier names; migrate or remove the old Feishu notifier only after
confirming codex-notify works.

The same applies to an existing transcript monitor or Stop Hook that sends
Feishu itself: keep it during migration if needed, but disable its Feishu send
path after codex-notify has been verified or interruption cards will duplicate.

## Interruption Detection

The background watcher checks only current-session directories near today and
recently modified archived transcripts. It stores byte offsets, so unchanged
JSONL records are not repeatedly read.

- A `task_complete.error` is persisted as `confirming` first, not sent at once.
- For ordinary turns, no resumed `task_started` for 30 seconds becomes an
  interruption card.
- For an active Goal, automatic continuation cancels the candidate. `complete`
  and `paused` cancel it; `blocked`, `usage_limited`, and `budget_limited`
  notify it; otherwise it requires 10 minutes without recovery.
- A normal Codex completion cancels any pending interruption candidate for that
  turn before the completion card is sent.

Run `codex-notify watch --once` for a manual scan. The installed background
service runs `codex-notify watch`, which scans once immediately and then every
30 seconds. `uninstall` removes that service, the Stop Hook, the prompt Hook,
and only codex-notify-managed local state.

## Privacy and Safety

- No hosted relay, telemetry, or analytics.
- App Secret is not written to configuration files or logs.
- Task text and final output are sent only to the selected Feishu receiver.
- Transcript-derived task context and pending-error state remain on the local
  machine under the codex-notify application-data directory.
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
