# codex-notify Specification

## 1. Purpose

`codex-notify` is an unofficial, local-first command-line tool that sends
actionable notifications for Codex work. It starts with Feishu private-message
notifications and is deliberately designed to support additional destinations
later.

The tool is installed once per user. It then participates in two local input
flows and one durable delivery flow:

```text
Codex completion notification -> codex-notify notify <event-json> -> local queue
Codex transcript completion/error records -> codex-notify watch -> local queue
local queue -> title resolution and deduplication -> provider
```

No application server, relay, analytics service, or shared database is part of
the product.

### Current M2 implementation

M1 completion cards are implemented end to end: Feishu authentication, secure
secret storage, Card JSON 2.0 rendering, prompt state capture, durable
background delivery, conversation-title lookup, duration formatting,
existing-notifier chaining, backup, and reversible uninstall.

M2 terminal-error monitoring is also implemented. It uses incremental JSONL
offsets, a durable two-stage confirmation state, transcript and Stop Hook
deduplication, a macOS per-user LaunchAgent, a Windows per-user startup entry,
and a Linux systemd user service. It is explicitly best-effort because
transcript JSONL is not a stable Codex extension interface.

## 2. Product Goals

1. Make it obvious from a phone notification whether a Codex task completed or
   failed.
2. Let a user configure Feishu and Codex through one interactive command.
3. Preserve existing user-level Codex notification behavior, including other
   notification programs already configured by the user.
4. Support macOS, Windows, and Linux from one Rust codebase and distribute
   standalone binaries through GitHub Releases.
5. Keep credentials out of plaintext configuration files.
6. Make error detection best-effort, explain its limits honestly, and avoid
   duplicate notifications.
7. Keep the core event model independent from Feishu so future providers do
   not require changes to Codex integration.

## 3. Non-goals for v1

- A GUI, menu-bar app, browser extension, or hosted dashboard.
- Team-wide administration or shared credential management.
- Cloud synchronization of task history.
- Guaranteed notification after a power loss, force quit, or OS crash.
- Modifying Codex source code or relying on undocumented network APIs.

## 4. Supported Platforms

| Platform | v1 support | Background integration |
| --- | --- | --- |
| macOS Apple Silicon | Required | M1 completion dispatcher and Hook; M2 LaunchAgent |
| macOS Intel | Required | M1 completion dispatcher and Hook; M2 LaunchAgent |
| Windows x64 | Required | M1 completion dispatcher and Hook; M2 startup entry |
| Linux ARM64 | Required | M1 completion dispatcher and Hook; M2 systemd user service |
| Linux x64 | Required | M1 completion dispatcher and Hook; M2 systemd user service |

The main binary must be self-contained. End users must not need Rust, Python,
Node.js, or a package manager after installation.

## 5. User Experience

### 5.1 Primary commands

```text
codex-notify init
codex-notify test
codex-notify status
codex-notify doctor
codex-notify sync
codex-notify uninstall
codex-notify watch
```

`init` is the primary onboarding path. It must be interactive by default and
keep command-line flags available for non-interactive automation.

### 5.2 `init` flow

1. Detect the operating system and existing Codex user configuration.
2. If codex-notify is already configured, show a non-secret summary and explain
   that initialization is not required. Default to keeping the current
   configuration; continue only after the user explicitly chooses to reconfigure.
3. Detect an existing `notify` command and existing `hooks.json` entries.
4. Explain exactly which files, credentials, and background task will be changed.
5. Ask for Feishu App ID, App Secret, receiver type, and receiver identifier.
6. Store the App Secret in the OS credential store.
7. Write non-secret configuration and keep a backup when replacing it.
8. Install the completion dispatcher, UserPromptSubmit Hook, and Stop Hook without
   deleting unrelated user configuration.
9. Install the platform background watcher without administrator privileges.
10. Send an opt-in test notification and report its result.
11. Explain that Codex requires the user to review and trust the new Hooks.
12. Print a concise status summary.

The interactive flow must use concise Chinese guidance. Before each value it
explains where to obtain the value and what format to expect. App Secret input
is hidden and followed by a safe acknowledgement that input was received. The
receiver type uses a keyboard selection menu with contextual descriptions.
Invalid input remains at the current prompt with a useful error instead of
terminating the entire initialization flow. The final write confirmation
defaults to yes because the preceding summary already explains every change and
the automatic backups. After installation, ChatGPT App users are told to open
Settings, enter Hooks, and trust the UserPromptSubmit and Stop Hooks in the user
section; Codex CLI users are told to run `/hooks` and trust the same two Hooks.

All human-facing command help, progress, status, diagnostics, update, uninstall,
installer, and error messages use concise Simplified Chinese. Stable command
names, option names, protocol values, and `--json` keys remain unchanged for
compatibility and automation.

The Stop Hook never sends an interruption card immediately. It only creates a
pending candidate, which the watcher confirms using the same rules as a
transcript error. This prevents a normal completion from racing a fallback
notification.

The installer must make a timestamped backup before modifying a user-managed
Codex configuration file. It must use a TOML/JSON parser and writer, not text
replacement.

### 5.3 Existing notify configuration

Codex's documented `notify` setting accepts one external command and invokes
it for `agent-turn-complete`. Users may already use that command for Computer
Use or another notifier.

`codex-notify init` must preserve the old command. A normal notifier becomes a
self-contained downstream command owned by the dispatcher. A recognized
Computer Use wrapper remains the outermost command, with `codex-notify notify`
stored in its `--previous-notify` value. The dispatcher must never forward back
to Computer Use.

The resulting chain is:

```text
Codex -> Computer Use (optional) -> codex-notify -> previous notifier (optional)
```

The dispatcher:

1. Persists the original completion event to the local delivery queue and
   returns without waiting for a title or provider network request.
2. Invokes the previous command with the original event input.
3. Captures failures independently so the previous destination cannot discard
   the queued provider notification.

The watcher is the only provider sender. It merges documented notify events
with best-effort completion records from local transcripts, resolves titles,
leases deliveries, retries transient failures, and deduplicates by turn ID.
This fallback allows a watcher upgrade to cover later completions from a
ChatGPT session that started before the notify configuration was installed.

Computer Use compatibility is guarded because `--previous-notify` is not a
documented Codex configuration interface. The parser must recognize only
tested macOS and Windows executable shapes, preserve unrelated wrapper
arguments, flatten duplicate Computer Use layers, and fail closed on malformed
or unknown Computer Use commands.

The background watcher must also reconcile notification integration after a
configuration manager switches the active `config.toml`. Each managed command
must carry its own previous notifier so profiles cannot use one another's
downstream command. Atomic writes must follow a symbolic link to its target
without replacing the link. `sync` exposes the same reconciliation explicitly.
The per-user background startup entry must persist the application directory
and `CODEX_HOME` explicitly on macOS, Windows, and Linux instead of relying on
a future login shell to recreate those environment variables.

`uninstall` must recognize both a direct dispatcher and one nested immediately
inside Computer Use. It restores each known managed configuration independently,
keeps Computer Use installed, and leaves unrecognized configurations untouched.
It must stop an already-running background watcher before restoring files so the
watcher cannot immediately write the managed chain back.

## 6. Notification Experience

### 6.1 Shared event model

The core produces a provider-neutral `Notification` value:

```text
Notification {
  outcome: Completed | Interrupted,
  conversation_title: String,
  task: String,
  details_markdown: String,
  elapsed: Option<Duration>,
  workspace: Option<PathBuf>,
  event_id: String,
  occurred_at: SystemTime,
}
```

Provider adapters receive this value. They may enforce destination-specific
limits, but may not change the outcome, event ID, or title semantics.

### 6.2 Mobile-first title

Many mobile notification trays show only the outer card title. Therefore the
title must always have a status prefix:

```text
Completed:   "OK HH:MM <conversation title>"
Interrupted: "WARN HH:MM <conversation title>"
```

The rendered Feishu v1 title uses the corresponding success or warning symbol.
The logical core keeps the outcome enum rather than storing presentation text.

If the local Codex conversation title cannot be resolved, the fallback title is
`Codex conversation`. The user prompt must not be used as the outer title.

### 6.3 Completion card

The initial Feishu card is an interactive Card JSON 2.0 card:

```text
Outer title:       success symbol + HH:MM + Codex conversation title
Collapsed header:  Codex task completed + Hh Mm Ss duration
Collapsed body:    local send time, task, then Markdown result
Color:             green
```

The task and result are collapsed by default. Markdown is preserved where the
destination supports it. The tool must account for the serialized card size,
not merely raw source-text byte count, and truncate only when necessary to fit
Feishu limits.

### 6.4 Interruption card

```text
Outer title:       warning symbol + HH:MM + Codex conversation title
Collapsed header:  Codex task interrupted + Hh Mm Ss duration
Collapsed body:    local send time, task, then error details and workspace
Color:             red
```

Completion and interruption cards use the same title, duration, collapse, and
Markdown conventions. The mobile-visible outer title is
`<status emoji> HH:MM <conversation title>`, and the collapsed Markdown body
starts with the local send date and time.

## 7. Codex Integration

### 7.1 Documented interfaces

The tool uses these Codex surfaces:

- User-level `notify` for `agent-turn-complete` events.
- User-level `UserPromptSubmit` hook to record task context and start time.
- User-level `Stop` hook as a best-effort fallback when no final message is
  available.

Codex provides common notification fields including the thread ID, turn ID,
working directory, user messages, and final assistant message. The tool must
gracefully handle missing optional fields.

### 7.2 Local state

For each turn, the prompt hook writes a small local state record keyed by a
hash of the turn ID:

```text
prompt
workspace
thread_id
conversation_title_at_start
started_at
```

The completion dispatcher and watcher read this record to calculate elapsed
time and obtain the original prompt. A normal completion removes the record
only after the provider acknowledges delivery. This ordering is required
because Stop, notify, transcript discovery, and provider delivery do not have a
guaranteed execution order.

State records are written atomically and have a bounded retention policy. The
implementation must prune abandoned records without touching user session
files.

### 7.3 Conversation title lookup

The local Codex session index can provide a human-readable thread title. The
lookup must scan from the most recent index entry and must have a safe fallback
when the index is absent or its format changes.

For a normal completion, if neither the current index nor the title captured at
turn start provides a title, the dispatcher first queues the event and exits.
The watcher retries the exact thread lookup for up to five seconds without
blocking Codex title generation. An already available title must not incur this
delay.

This lookup is an enhancement rather than a hard dependency. The tool must not
fail a notification solely because a title cannot be found.

## 8. Error Detection

### 8.1 Sources

Normal completion primarily comes from the documented `notify` event.
The watcher also recognizes normal `task_complete` records as a best-effort
fallback for sessions that did not load the current notify configuration.
Terminal-error coverage needs the same local watcher because `notify` currently
emits only completion events.

The watcher reads only recent Codex session transcript files and tracks byte
offsets. On a state-schema upgrade it performs a bounded ten-minute lookback,
seeding previous documented completions as delivered so the migration does not
duplicate cards. It watches terminal `task_complete` records with an error
message, including examples such as:

- Stream disconnects and transport failures.
- Provider or proxy failures.
- Authentication failures.
- Usage or quota-limit failures.
- Server-side terminal errors.

An exact known usage-limit terminal message is also recognized if a provider
surfaces it as the only final message rather than in an error field.

### 8.2 Two-stage confirmation

On discovery, a terminal error is written to durable `confirming` state rather
than sent immediately. The watcher scans the rest of the same transcript before
making any decision, then scans only later bytes on subsequent passes.

- Ordinary turns notify only after 30 seconds without a later `task_started`.
- Any later `task_started` cancels the candidate because a reconnect, manual
  continuation, or Goal continuation started work again.
- If the Goal was `active` at the error, the candidate is not notified for an
  individual failed turn. `complete` and `paused` cancel it; `blocked`,
  `usage_limited`, and `budget_limited` notify it; otherwise it becomes due
  after 10 minutes of silence.
- A Stop Hook with no final assistant message creates the same kind of pending
  candidate. A documented normal completion cancels candidates for its turn.

### 8.3 Limitations

Codex transcript format is not a stable public hook interface. The watcher is
therefore best-effort and must isolate parsing behind a `TranscriptSource`
interface with fixture coverage for supported observed records.

The tool cannot guarantee an alert if Codex, the binary, or the operating
system is terminated before a terminal record is written. It must surface this
limitation in `doctor` and documentation.

### 8.4 Deduplication

Each terminal event has a stable digest built from its turn ID, completion
timestamp, and normalized error message. Normal completions use the turn ID to
merge the notify and transcript paths before delivery. The watcher persists
bounded delivered-turn and terminal-event histories. State shared with the Stop
fallback prevents completion, Stop, and transcript paths from notifying the
same turn twice.

The default retention target is 4,096 recent event digests and 24 hours for
turn-state records. Both are configurable within documented safe bounds.

## 9. Feishu v1 Provider

### 9.1 Required user configuration

The user creates and configures a Feishu bot application. `init` asks for:

```text
app_id
app_secret
receiver_id_type: open_id | user_id | email | chat_id
receiver_id
```

The tool obtains a tenant access token locally and sends messages directly to
the Feishu Open API. It does not proxy the credentials through a third-party
service.

### 9.2 Credential storage

| Value | Storage |
| --- | --- |
| App Secret | macOS Keychain, Windows Credential Manager, or Linux Secret Service |
| App ID and receiver selection | Local non-secret config file |
| Access token | Memory only |

The Rust implementation should keep credential access behind a `SecretStore`
trait. Windows and Linux use the native backends of a cross-platform credential
library. macOS writes through the native Keychain API and reads through the
Apple-signed, stable `/usr/bin/security` helper so a self-update does not bind
access to one release binary's cdhash. A secret must never be passed through
command-line arguments. Upgrades from the legacy direct-Keychain backend must
migrate access in the foreground before restarting the watcher. `doctor` may
verify that a secret exists but must never print it.

### 9.3 API behavior

- Request a tenant access token only when needed.
- Use bounded request timeouts and clear redacted error messages.
- Retry only safe transient failures with a small, bounded backoff.
- Do not retry a message after Feishu has acknowledged it.
- Enforce destination card-size limits before sending.

## 10. Future Providers

Feishu is the only provider in v1. The architecture reserves provider adapters
for, but does not implement, destinations such as Slack, Discord, Telegram,
ntfy, generic webhooks, email, or native desktop notifications.

```text
trait NotificationProvider {
  fn name(&self) -> &'static str;
  fn validate(&self) -> ProviderStatus;
  fn send(&self, notification: &Notification) -> Result<DeliveryReceipt>;
}
```

Provider configuration must be namespaced. Adding a provider must not require
changing the event model, Codex dispatcher, watcher, or existing Feishu
configuration.

## 11. Local File Layout

The exact platform-specific base directory follows native conventions:

```text
macOS:   ~/Library/Application Support/codex-notify/
Windows: %APPDATA%\\codex-notify\\
```

Under that base directory:

```text
config.toml          Non-secret provider and behavior configuration
state/               Atomic, short-lived turn and deduplication state
logs/                Capped diagnostics log
backups/             Installer-created configuration backups
```

Logs are capped by byte size and rotate by retaining the newest tail. They must
not include App Secrets, access tokens, or unredacted authorization headers.

## 12. Platform Services

### 12.1 macOS

`init` installs a per-user LaunchAgent at
`~/Library/LaunchAgents/com.codex-notify.watcher.plist`. It runs
`codex-notify watch` at login, keeps it alive after an unexpected exit, and
uses the capped application diagnostics log rather than unbounded service
stdout/stderr logs. No administrator privileges are required.

### 12.2 Windows

`init` creates a per-user `CodexNotifyWatcher` value under the current user's
Windows `Run` registry key. It starts `codex-notify watch` in a hidden window
at logon and does not require a system-wide service or administrator
privileges. The watcher handles scan failures inside its long-running process;
an unexpected process exit is recovered at the next login.

### 12.3 Linux

`init` installs a per-user systemd unit at
`~/.config/systemd/user/codex-notify-watcher.service`. It starts
`codex-notify watch` with the user session, restarts after unexpected failures,
and does not require a system-wide service or administrator privileges. Linux
credential storage uses the desktop Secret Service and must never fall back to
a plaintext file.

### 12.4 Watcher behavior

The watcher is a low-resource long-running process. It scans immediately and
then sleeps for 30 seconds, uses offsets instead of re-reading full JSONL
files, looks only in session directories around the current day plus recently
modified archived files, limits persistent candidate state, and handles file
rotation or truncation safely.

## 13. Security and Privacy

- No telemetry, tracking, or analytics by default.
- No relay service.
- Prompt text and final output leave the machine only for the Feishu recipient
  selected by the user.
- Secrets are stored in OS credential storage.
- Configuration writes are explicit, backed up, and reversible.
- `doctor` redacts credentials and sensitive identifiers in human output.
- Release artifacts include SHA-256 checksums.
- Future signed releases should add macOS notarization and Windows code signing.

## 14. CLI Exit Codes

| Code | Meaning |
| --- | --- |
| 0 | Command completed successfully. |
| 1 | Operational failure, such as provider or watcher failure. |
| 2 | Invalid arguments, incomplete configuration, or unsupported environment. |
| 3 | User declined a requested configuration change. |

Machine-readable output via `--json` is required for `status`, `doctor`, and
future non-interactive installation flows.

## 15. Rust Architecture

```text
src/
  main.rs             CLI entry point
  commands/           init, test, status, doctor, uninstall, watch, notify
  codex/              config integration, hooks, event parsing, state
  core/               notification model, duration, redaction, deduplication
  providers/          provider trait and Feishu adapter
  platform/           macOS, Windows, and Linux background-service adapters
  secrets/            credential-store abstraction
  transcripts/        incremental transcript sources and fixtures
```

Recommended Rust boundaries:

- `clap` for stable CLI parsing and generated help.
- `serde`, `serde_json`, and `toml_edit` for structured data/configuration.
- `keyring` or an equivalent OS credential-store abstraction.
- `reqwest` with explicit timeouts for Feishu requests.
- `tokio` only where asynchronous I/O provides a clear benefit.
- `thiserror` for user-facing typed errors.

The core and provider adapters must be testable without the live Codex app,
Feishu API, Keychain, Credential Manager, LaunchAgent, or Windows startup.

## 16. Verification Requirements

Every release must pass:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Tests must include:

- Completion card and interruption card fixtures.
- Mobile-title success and warning prefixes.
- Duration formatting across seconds, minutes, and hours.
- Card serialized-size boundaries including JSON escaping.
- Existing `notify` chaining and exact restoration on uninstall.
- Existing Hook configuration merge behavior.
- Transcript offset, rotation, partial-line, and deduplication behavior.
- Stream-disconnect and usage-limit fixtures.
- Secret-redaction assertions.
- Platform installer command generation.

CI must run lint and tests on macOS, Windows, and Linux. Release CI must build:

```text
x86_64-apple-darwin
aarch64-apple-darwin
x86_64-pc-windows-msvc
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
```

## 17. Distribution and Releases

GitHub Releases is the source of truth for first-party artifacts. Pushing a
signed-off tag matching `v*` triggers the release workflow.

Each release publishes:

```text
codex-notify-x86_64-apple-darwin.tar.gz
codex-notify-aarch64-apple-darwin.tar.gz
codex-notify-x86_64-pc-windows-msvc.zip
codex-notify-x86_64-unknown-linux-gnu.tar.gz
codex-notify-aarch64-unknown-linux-gnu.tar.gz
SHA256SUMS
```

The initial installation channels are:

1. GitHub Release download with documented verification and first-party
   `install.sh` and `install.ps1` scripts. The scripts select the host target,
   fetch the latest release by its stable asset name, verify `SHA256SUMS`, and
   install only into a user-writable directory. They must not modify Codex,
   Feishu, shell startup files, or system-wide directories.
2. A Homebrew tap for macOS.
3. A Scoop manifest for Windows.

Installed standalone binaries expose `codex-notify update`. The command and
the first-party install scripts share one update transaction: resolve a
release, verify its `SHA256SUMS` entry and staged version, stop (but do not
uninstall) the watcher, replace the executable, refresh the existing
integration without reading or replacing the Feishu secret, restart the
watcher, and verify the new process. Any failure after replacement restores
the previous executable and watcher. Re-running an install script delegates to
this command when available; a downloaded newer binary performs the same
transaction when upgrading a legacy installation.

Update networking accepts an explicit `--proxy` or `--no-proxy`, otherwise it
uses standard proxy environment variables before the enabled per-user Windows
manual system proxy. Connection failures must identify whether a proxy or
direct route was attempted and give an actionable recovery without naming a
specific proxy product.
The Windows installer must pass an enabled manual system proxy to older
installed updaters so users can bootstrap into this behavior without editing
terminal environment variables.

winget and crates.io publication are later milestones, after the installer,
upgrade, signing, and compatibility behavior are stable.

## 18. Milestones

### M0: Foundation

- Rust crate, README, MIT license, specification, CI, and release workflow.

### M1: Feishu completion notifications

- Feishu configuration and secure secret storage.
- Card renderer and `notify` dispatcher.
- Existing notify-command preservation.
- `init`, `test`, `status`, and `uninstall`.

### M2: Error watcher

- Implemented: incremental transcript watcher, two-stage confirmation, Stop
  fallback handling, macOS LaunchAgent and Windows startup integration,
  stream-disconnect and usage-limit fixtures, `doctor`, and deduplication.

### M3: Distribution hardening

- Release checksums, Homebrew, Scoop, upgrade path, migration behavior.
- macOS notarization and Windows signing evaluation.

### M4: Additional providers

- Provider-adapter contract proven by a second destination.

## 19. Decisions to Preserve

- Feishu is the only implemented v1 provider.
- The project is local-first and has no hosted backend.
- The binary must be usable after one interactive `init` command.
- The installer must preserve existing Codex integration.
- Status must be visible in the mobile title, not only inside a card.
- Error detection is useful but explicitly best-effort.
- All persistent installation changes must be inspectable and reversible.
