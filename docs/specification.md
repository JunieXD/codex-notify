# codex-notify Specification

## 1. Purpose

`codex-notify` is an unofficial, local-first command-line tool that sends
actionable notifications for Codex work. It starts with Feishu private-message
notifications and is deliberately designed to support additional destinations
later.

The tool is installed once per user. It then participates in two local flows:

```text
Codex completion notification -> codex-notify notify <event-json>
Codex transcript/error watcher -> codex-notify watch
```

No application server, relay, analytics service, or shared database is part of
the product.

## 2. Product Goals

1. Make it obvious from a phone notification whether a Codex task completed or
   failed.
2. Let a user configure Feishu and Codex through one interactive command.
3. Preserve existing user-level Codex notification behavior, including other
   notification programs already configured by the user.
4. Support macOS and Windows from one Rust codebase and distribute standalone
   binaries through GitHub Releases.
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
- Supporting Linux in the first release. The architecture must not preclude it.

## 4. Supported Platforms

| Platform | v1 support | Background integration |
| --- | --- | --- |
| macOS Apple Silicon | Required | Per-user LaunchAgent |
| macOS Intel | Required | Per-user LaunchAgent |
| Windows x64 | Required | Per-user Task Scheduler task |

The main binary must be self-contained. End users must not need Rust, Python,
Node.js, or a package manager after installation.

## 5. User Experience

### 5.1 Primary commands

```text
codex-notify init
codex-notify test
codex-notify status
codex-notify doctor
codex-notify uninstall
codex-notify watch
```

`init` is the primary onboarding path. It must be interactive by default and
must support a documented non-interactive mode later for automation.

### 5.2 `init` flow

1. Detect the operating system and existing Codex user configuration.
2. Detect an existing `notify` command and existing `hooks.json` entries.
3. Explain exactly which files and background task will be changed.
4. Ask for Feishu App ID, App Secret, receiver type, and receiver identifier.
5. Store the App Secret in the OS credential store.
6. Write non-secret configuration.
7. Install the completion dispatcher and the UserPromptSubmit/Stop hooks
   without deleting unrelated user configuration.
8. Install and start the local error watcher.
9. Send an opt-in test notification and report its result.
10. Run `doctor` automatically and print a concise summary.

The installer must make a timestamped backup before modifying a user-managed
Codex configuration file. It must use a TOML/JSON parser and writer, not text
replacement.

### 5.3 Existing notify configuration

Codex's documented `notify` setting accepts one external command and invokes
it for `agent-turn-complete`. Users may already use that command for Computer
Use or another notifier.

`codex-notify init` must not overwrite the old command. It must record the
previous command and install a dispatcher that:

1. Invokes the previous command with the original event input.
2. Invokes `codex-notify notify` with the same event input.
3. Captures failures independently so one notifier does not suppress the
   other.

`uninstall` must restore the previous command only when the active dispatcher
is demonstrably owned by `codex-notify`. It must otherwise leave the user's
configuration untouched and report the manual action needed.

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
Completed:   "OK <conversation title>"
Interrupted: "WARN <conversation title>"
```

The rendered Feishu v1 title uses the corresponding success or warning symbol.
The logical core keeps the outcome enum rather than storing presentation text.

If the local Codex conversation title cannot be resolved, the fallback title is
`Codex conversation`. The user prompt must not be used as the outer title.

### 6.3 Completion card

The initial Feishu card is an interactive Card JSON 2.0 card:

```text
Outer title:       success symbol + Codex conversation title
Collapsed header:  Codex task completed + Hh Mm Ss duration
Collapsed body:    task, then Markdown result
Color:             green
```

The task and result are collapsed by default. Markdown is preserved where the
destination supports it. The tool must account for the serialized card size,
not merely raw source-text byte count, and truncate only when necessary to fit
Feishu limits.

### 6.4 Interruption card

```text
Outer title:       warning symbol + Codex conversation title
Collapsed header:  Codex task interrupted + Hh Mm Ss duration
Collapsed body:    task, then error details and workspace when available
Color:             red
```

Completion and interruption cards must use the same title, duration, collapse,
and Markdown conventions.

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

The completion dispatcher reads this record to calculate elapsed time and
obtain the original prompt. A normal completion removes the record only after
the notification dispatch has used it. This ordering is required because Stop
and notify do not have a guaranteed execution order.

State records are written atomically and have a bounded retention policy. The
implementation must prune abandoned records without touching user session
files.

### 7.3 Conversation title lookup

The local Codex session index can provide a human-readable thread title. The
lookup must scan from the most recent index entry and must have a safe fallback
when the index is absent or its format changes.

This lookup is an enhancement rather than a hard dependency. The tool must not
fail a notification solely because a title cannot be found.

## 8. Error Detection

### 8.1 Sources

Normal completion comes from the documented `notify` event. Terminal-error
coverage needs a local watcher because `notify` currently emits only completion
events.

The watcher reads only recently changed Codex session transcript files and
tracks byte offsets. It watches terminal `task_complete` records with an error
message, including examples such as:

- Stream disconnects and transport failures.
- Provider or proxy failures.
- Authentication failures.
- Usage or quota-limit failures.
- Server-side terminal errors.

An exact known usage-limit terminal message is also recognized if a provider
surfaces it as the only final message rather than in an error field.

### 8.2 Limitations

Codex transcript format is not a stable public hook interface. The watcher is
therefore best-effort and must isolate parsing behind a `TranscriptSource`
interface with fixture coverage for supported observed records.

The tool cannot guarantee an alert if Codex, the binary, or the operating
system is terminated before a terminal record is written. It must surface this
limitation in `doctor` and documentation.

### 8.3 Deduplication

Each terminal event has a stable digest built from its turn ID, completion
timestamp, and normalized error message. The watcher persists a bounded set of
recent digests. State shared with the Stop fallback prevents a Stop alert and a
watcher alert from notifying the same turn twice.

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
| App Secret | macOS Keychain or Windows Credential Manager |
| App ID and receiver selection | Local non-secret config file |
| Access token | Memory only |

The Rust implementation should use a cross-platform credential-store library
behind a `SecretStore` trait. `doctor` may verify that a secret exists but must
never print it.

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

`init` installs a per-user LaunchAgent. It runs `codex-notify watch` at login,
restarts it after an unexpected exit, and writes logs to the application data
directory. No administrator privileges are required.

### 12.2 Windows

`init` creates a per-user Task Scheduler task triggered at logon. The task runs
`codex-notify watch`, restarts on failure, and does not require a system-wide
service or administrator privileges.

### 12.3 Watcher behavior

The watcher is a low-resource long-running process. It uses offsets instead of
re-reading full JSONL files, sleeps while idle, limits memory use, and handles
file rotation or truncation safely.

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
  platform/           macOS LaunchAgent and Windows Task Scheduler adapters
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
Feishu API, Keychain, Credential Manager, LaunchAgent, or Task Scheduler.

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

CI must run lint and tests on macOS and Windows. Release CI must build:

```text
x86_64-apple-darwin
aarch64-apple-darwin
x86_64-pc-windows-msvc
```

## 17. Distribution and Releases

GitHub Releases is the source of truth for first-party artifacts. Pushing a
signed-off tag matching `v*` triggers the release workflow.

Each release publishes:

```text
codex-notify-x86_64-apple-darwin.tar.gz
codex-notify-aarch64-apple-darwin.tar.gz
codex-notify-x86_64-pc-windows-msvc.zip
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

- Incremental transcript watcher.
- macOS LaunchAgent and Windows Task Scheduler integration.
- Stream-disconnect and usage-limit fixtures.
- `doctor` and deduplication.

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
