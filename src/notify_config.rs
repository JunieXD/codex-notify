use anyhow::{Context, Result, bail};
use std::path::Path;

pub const MANAGED_NOTIFY_FLAG: &str = "--managed";
pub const FORWARD_NOTIFY_FLAG: &str = "--forward-notify";
const COMPUTER_USE_PREVIOUS_FLAG: &str = "--previous-notify";
const TURN_ENDED_SUBCOMMAND: &str = "turn-ended";
const MAX_COMPUTER_USE_LAYERS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyPlacement {
    Direct,
    ComputerUse,
}

impl NotifyPlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ComputerUse => "via_computer_use",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyPlan {
    pub active_command: Vec<String>,
    pub managed_command: Vec<String>,
    pub previous_notify: Option<Vec<String>>,
    pub placement: NotifyPlacement,
    pub owned_before: bool,
    pub legacy_owned_before: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyRemoval {
    pub restored_command: Option<Vec<String>>,
    pub previous_notify: Option<Vec<String>>,
    pub placement: NotifyPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedCommand {
    self_contained: bool,
    previous_notify: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComputerUseCommand {
    command: Vec<String>,
    previous_flag_index: Option<usize>,
    previous_notify: Option<Vec<String>>,
}

pub fn managed_notify_command(binary: &Path) -> Vec<String> {
    vec![
        binary.to_string_lossy().into_owned(),
        "notify".to_owned(),
        MANAGED_NOTIFY_FLAG.to_owned(),
    ]
}

pub fn managed_programs(
    canonical_managed: &[String],
    historical_programs: &[String],
) -> Vec<String> {
    let mut programs = Vec::new();
    if let Some(program) = canonical_managed.first() {
        programs.push(program.clone());
    }
    for program in historical_programs {
        if !program.trim().is_empty() && !programs.contains(program) {
            programs.push(program.clone());
        }
    }
    programs
}

pub fn plan_notify_integration(
    active: Option<Vec<String>>,
    canonical_managed: &[String],
    managed_programs: &[String],
    legacy_previous: Option<&[String]>,
) -> Result<NotifyPlan> {
    validate_canonical_managed(canonical_managed)?;
    let (outer, leaf) = split_computer_use_layers(active)?;
    let mut owned_before = false;
    let mut legacy_owned_before = false;
    let previous = match leaf {
        Some(command) => match parse_managed_command(&command, managed_programs)? {
            Some(managed) => {
                owned_before = true;
                legacy_owned_before = !managed.self_contained;
                if managed.self_contained {
                    managed.previous_notify
                } else {
                    legacy_previous.map(<[String]>::to_vec)
                }
            }
            None => {
                reject_foreign_codex_notify(&command)?;
                Some(command)
            }
        },
        None => None,
    };
    let previous = normalize_downstream(previous, managed_programs)?;
    let managed_command = with_forward_notify(canonical_managed, previous.as_deref())?;
    let (active_command, placement) = match outer {
        Some(wrapper) => (
            wrapper.with_previous(Some(managed_command.clone()))?,
            NotifyPlacement::ComputerUse,
        ),
        None => (managed_command.clone(), NotifyPlacement::Direct),
    };

    Ok(NotifyPlan {
        active_command,
        managed_command,
        previous_notify: previous,
        placement,
        owned_before,
        legacy_owned_before,
    })
}

pub fn inspect_notify_integration(
    active: Option<Vec<String>>,
    managed_programs: &[String],
) -> Result<Option<NotifyPlacement>> {
    let (outer, leaf) = split_computer_use_layers(active)?;
    let Some(leaf) = leaf else {
        return Ok(None);
    };
    if parse_managed_command(&leaf, managed_programs)?.is_none() {
        return Ok(None);
    }
    Ok(Some(if outer.is_some() {
        NotifyPlacement::ComputerUse
    } else {
        NotifyPlacement::Direct
    }))
}

pub fn remove_notify_integration(
    active: Option<Vec<String>>,
    managed_programs: &[String],
    legacy_previous: Option<&[String]>,
) -> Result<Option<NotifyRemoval>> {
    let (outer, leaf) = split_computer_use_layers(active)?;
    let Some(leaf) = leaf else {
        return Ok(None);
    };
    let Some(managed) = parse_managed_command(&leaf, managed_programs)? else {
        return Ok(None);
    };
    let previous = if managed.self_contained {
        managed.previous_notify
    } else {
        legacy_previous.map(<[String]>::to_vec)
    };
    let previous = normalize_downstream(previous, managed_programs)?;
    let (restored_command, placement) = match outer {
        Some(wrapper) => (
            Some(wrapper.with_previous(previous.clone())?),
            NotifyPlacement::ComputerUse,
        ),
        None => (previous.clone(), NotifyPlacement::Direct),
    };
    Ok(Some(NotifyRemoval {
        restored_command,
        previous_notify: previous,
        placement,
    }))
}

pub fn parse_forward_notify(value: &str) -> Result<Vec<String>> {
    parse_command_json(value, "codex-notify forwarded notifier")
}

fn validate_canonical_managed(command: &[String]) -> Result<()> {
    if command.len() != 3
        || command[0].trim().is_empty()
        || command[1] != "notify"
        || command[2] != MANAGED_NOTIFY_FLAG
    {
        bail!("managed codex-notify command has an unsupported shape");
    }
    Ok(())
}

fn with_forward_notify(
    canonical_managed: &[String],
    previous: Option<&[String]>,
) -> Result<Vec<String>> {
    validate_canonical_managed(canonical_managed)?;
    let mut command = canonical_managed.to_vec();
    if let Some(previous) = previous {
        if previous.is_empty() {
            bail!("cannot forward to an empty notifier command");
        }
        command.push(FORWARD_NOTIFY_FLAG.to_owned());
        command.push(
            serde_json::to_string(previous)
                .context("could not serialize the forwarded notifier command")?,
        );
    }
    Ok(command)
}

fn parse_managed_command(
    command: &[String],
    managed_programs: &[String],
) -> Result<Option<ManagedCommand>> {
    if command.len() < 2
        || command[1] != "notify"
        || !managed_programs
            .iter()
            .any(|program| program == &command[0])
    {
        return Ok(None);
    }

    let mut managed_marker = false;
    let mut forward = None;
    let mut index = 2;
    while index < command.len() {
        match command[index].as_str() {
            MANAGED_NOTIFY_FLAG => {
                if managed_marker {
                    bail!(
                        "managed codex-notify command contains a duplicate {MANAGED_NOTIFY_FLAG}"
                    );
                }
                managed_marker = true;
                index += 1;
            }
            FORWARD_NOTIFY_FLAG => {
                if forward.is_some() {
                    bail!(
                        "managed codex-notify command contains a duplicate {FORWARD_NOTIFY_FLAG}"
                    );
                }
                let value = command
                    .get(index + 1)
                    .context("managed codex-notify command is missing its forwarded notifier")?;
                forward = Some(parse_forward_notify(value)?);
                index += 2;
            }
            argument => {
                bail!("managed codex-notify command contains unsupported argument '{argument}'");
            }
        }
    }

    Ok(Some(ManagedCommand {
        self_contained: managed_marker || forward.is_some(),
        previous_notify: forward,
    }))
}

fn split_computer_use_layers(
    active: Option<Vec<String>>,
) -> Result<(Option<ComputerUseCommand>, Option<Vec<String>>)> {
    let mut outer = None;
    let mut current = active;
    let mut depth = 0usize;
    while let Some(command) = current {
        let Some(wrapper) = ComputerUseCommand::parse(&command)? else {
            return Ok((outer, Some(command)));
        };
        depth += 1;
        if depth > MAX_COMPUTER_USE_LAYERS {
            bail!("Computer Use notify nesting exceeds the supported limit");
        }
        current = wrapper.previous_notify.clone();
        if outer.is_none() {
            outer = Some(wrapper);
        }
    }
    Ok((outer, None))
}

fn normalize_downstream(
    previous: Option<Vec<String>>,
    managed_programs: &[String],
) -> Result<Option<Vec<String>>> {
    let (_, leaf) = split_computer_use_layers(previous)?;
    if let Some(command) = leaf.as_ref() {
        if parse_managed_command(command, managed_programs)?.is_some() {
            bail!("refusing to create a recursive codex-notify command");
        }
        reject_foreign_codex_notify(command)?;
    }
    Ok(leaf)
}

fn reject_foreign_codex_notify(command: &[String]) -> Result<()> {
    if command.len() >= 2
        && command[1] == "notify"
        && executable_name(&command[0]).is_some_and(|name| {
            name.eq_ignore_ascii_case("codex-notify")
                || name.eq_ignore_ascii_case("codex-notify.exe")
        })
    {
        bail!(
            "another codex-notify command is already configured; refusing to create a recursive chain"
        );
    }
    Ok(())
}

impl ComputerUseCommand {
    fn parse(command: &[String]) -> Result<Option<Self>> {
        if command.len() < 2 || command[1] != TURN_ENDED_SUBCOMMAND {
            return Ok(None);
        }
        let Some(name) = executable_name(&command[0]) else {
            return Ok(None);
        };
        if !name.eq_ignore_ascii_case("SkyComputerUseClient")
            && !name.eq_ignore_ascii_case("codex-computer-use.exe")
        {
            let normalized = name.to_ascii_lowercase();
            if normalized.contains("computeruse") || normalized.contains("computer-use") {
                bail!(
                    "unsupported Computer Use notify executable '{name}'; refusing to rewrite it"
                );
            }
            return Ok(None);
        }

        let mut previous_flag_index = None;
        let mut previous_notify = None;
        let mut index = 2;
        while index < command.len() {
            if command[index] == COMPUTER_USE_PREVIOUS_FLAG {
                if previous_flag_index.is_some() {
                    bail!(
                        "Computer Use notify command contains a duplicate {COMPUTER_USE_PREVIOUS_FLAG}"
                    );
                }
                let value = command
                    .get(index + 1)
                    .context("Computer Use notify command is missing its previous notifier")?;
                previous_notify =
                    Some(parse_command_json(value, "Computer Use previous notifier")?);
                previous_flag_index = Some(index);
                index += 2;
            } else {
                index += 1;
            }
        }

        Ok(Some(Self {
            command: command.to_vec(),
            previous_flag_index,
            previous_notify,
        }))
    }

    fn with_previous(&self, previous: Option<Vec<String>>) -> Result<Vec<String>> {
        let mut command = self.command.clone();
        match (self.previous_flag_index, previous) {
            (Some(index), Some(previous)) => {
                command[index + 1] = serde_json::to_string(&previous)
                    .context("could not serialize Computer Use previous notifier")?;
            }
            (Some(index), None) => {
                command.drain(index..=index + 1);
            }
            (None, Some(previous)) => {
                command.push(COMPUTER_USE_PREVIOUS_FLAG.to_owned());
                command.push(
                    serde_json::to_string(&previous)
                        .context("could not serialize Computer Use previous notifier")?,
                );
            }
            (None, None) => {}
        }
        Ok(command)
    }
}

fn parse_command_json(value: &str, label: &str) -> Result<Vec<String>> {
    let command: Vec<String> = serde_json::from_str(value)
        .with_context(|| format!("{label} must be a JSON string array"))?;
    if command.is_empty() || command[0].trim().is_empty() {
        bail!("{label} must not be empty");
    }
    Ok(command)
}

fn executable_name(program: &str) -> Option<&str> {
    program.rsplit(['/', '\\']).find(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        FORWARD_NOTIFY_FLAG, MANAGED_NOTIFY_FLAG, NotifyPlacement, inspect_notify_integration,
        managed_notify_command, plan_notify_integration, remove_notify_integration,
    };
    use std::path::Path;

    fn managed() -> Vec<String> {
        managed_notify_command(Path::new("/opt/codex-notify"))
    }

    fn programs() -> Vec<String> {
        vec!["/opt/codex-notify".to_owned()]
    }

    fn mac_computer_use(previous: Option<Vec<String>>) -> Vec<String> {
        let mut command = vec![
            "/Users/test/.codex/computer-use/SkyComputerUseClient".to_owned(),
            "turn-ended".to_owned(),
        ];
        if let Some(previous) = previous {
            command.push("--previous-notify".to_owned());
            command.push(serde_json::to_string(&previous).expect("serialize previous"));
        }
        command
    }

    fn windows_computer_use(previous: Option<Vec<String>>) -> Vec<String> {
        let mut command = vec![
            r"C:\Users\test\.codex\computer-use\codex-computer-use.exe".to_owned(),
            "turn-ended".to_owned(),
        ];
        if let Some(previous) = previous {
            command.push("--previous-notify".to_owned());
            command.push(serde_json::to_string(&previous).expect("serialize previous"));
        }
        command
    }

    fn embedded_previous(command: &[String], flag: &str) -> Option<Vec<String>> {
        let index = command.iter().position(|argument| argument == flag)?;
        serde_json::from_str(command.get(index + 1)?).ok()
    }

    #[test]
    fn installs_inside_macos_computer_use_without_forwarding_back_to_it() {
        let active = mac_computer_use(None);
        let plan = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect("plan integration");

        assert_eq!(plan.placement, NotifyPlacement::ComputerUse);
        assert_eq!(plan.previous_notify, None);
        assert_eq!(
            embedded_previous(&plan.active_command, "--previous-notify"),
            Some(managed())
        );
        assert!(
            !plan
                .managed_command
                .contains(&FORWARD_NOTIFY_FLAG.to_owned())
        );
    }

    #[test]
    fn preserves_computer_use_previous_notifier_inside_our_dispatcher() {
        let previous = vec!["python3".to_owned(), "/tmp/notify.py".to_owned()];
        let active = mac_computer_use(Some(previous.clone()));
        let plan = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect("plan integration");

        assert_eq!(plan.previous_notify, Some(previous.clone()));
        assert_eq!(
            embedded_previous(&plan.managed_command, FORWARD_NOTIFY_FLAG),
            Some(previous)
        );
        assert_eq!(
            embedded_previous(&plan.active_command, "--previous-notify"),
            Some(plan.managed_command)
        );
    }

    #[test]
    fn app_rewrap_is_idempotent_and_does_not_add_an_inner_computer_use() {
        let previous = vec!["old-notifier".to_owned()];
        let initial = plan_notify_integration(
            Some(mac_computer_use(Some(previous.clone()))),
            &managed(),
            &programs(),
            None,
        )
        .expect("initial plan");
        let app_rewrapped = mac_computer_use(Some(initial.managed_command.clone()));
        let repaired = plan_notify_integration(
            Some(app_rewrapped),
            &managed(),
            &programs(),
            Some(mac_computer_use(Some(previous.clone())).as_slice()),
        )
        .expect("repair plan");

        assert!(repaired.owned_before);
        assert_eq!(repaired.previous_notify, Some(previous));
        assert_eq!(repaired.active_command, initial.active_command);
    }

    #[test]
    fn nested_computer_use_wrappers_are_flattened_to_the_outer_current_wrapper() {
        let previous = vec!["old-notifier".to_owned()];
        let nested = mac_computer_use(Some(mac_computer_use(Some(previous.clone()))));
        let plan = plan_notify_integration(Some(nested), &managed(), &programs(), None)
            .expect("flatten nested wrappers");

        let managed = embedded_previous(&plan.active_command, "--previous-notify")
            .expect("outer Computer Use previous");
        assert_eq!(
            embedded_previous(&managed, FORWARD_NOTIFY_FLAG),
            Some(previous)
        );
        assert_eq!(
            managed
                .iter()
                .filter(|argument| argument.as_str() == "turn-ended")
                .count(),
            0
        );
    }

    #[test]
    fn unrelated_computer_use_arguments_survive_reconciliation() {
        let mut active = mac_computer_use(Some(vec!["old-notifier".to_owned()]));
        active.splice(2..2, ["--transport".to_owned(), "xpc".to_owned()]);
        active.extend(["--quiet".to_owned()]);

        let plan = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect("plan with extra arguments");

        assert!(
            plan.active_command
                .windows(2)
                .any(|arguments| { arguments == ["--transport".to_owned(), "xpc".to_owned()] })
        );
        assert!(plan.active_command.contains(&"--quiet".to_owned()));
    }

    #[test]
    fn migrates_v2_legacy_state_that_saved_computer_use_as_previous() {
        let old_managed = vec!["/opt/codex-notify".to_owned(), "notify".to_owned()];
        let active = mac_computer_use(Some(old_managed));
        let legacy_previous = mac_computer_use(None);
        let plan = plan_notify_integration(
            Some(active),
            &managed(),
            &programs(),
            Some(&legacy_previous),
        )
        .expect("migration plan");

        assert_eq!(plan.previous_notify, None);
        assert!(
            !plan
                .managed_command
                .contains(&FORWARD_NOTIFY_FLAG.to_owned())
        );
    }

    #[test]
    fn supports_windows_computer_use_paths() {
        let active = windows_computer_use(Some(vec!["notify.exe".to_owned()]));
        let plan = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect("Windows plan");
        assert_eq!(plan.placement, NotifyPlacement::ComputerUse);
        assert_eq!(
            inspect_notify_integration(Some(plan.active_command), &programs()).expect("inspect"),
            Some(NotifyPlacement::ComputerUse)
        );
    }

    #[test]
    fn uninstall_keeps_computer_use_and_restores_its_previous_notifier() {
        let previous = vec!["other-notifier".to_owned(), "--quiet".to_owned()];
        let plan = plan_notify_integration(
            Some(mac_computer_use(Some(previous.clone()))),
            &managed(),
            &programs(),
            None,
        )
        .expect("install plan");
        let removal = remove_notify_integration(Some(plan.active_command), &programs(), None)
            .expect("remove integration")
            .expect("owned integration");

        assert_eq!(removal.placement, NotifyPlacement::ComputerUse);
        assert_eq!(removal.previous_notify, Some(previous.clone()));
        assert_eq!(
            embedded_previous(
                removal
                    .restored_command
                    .as_deref()
                    .expect("Computer Use remains"),
                "--previous-notify"
            ),
            Some(previous)
        );
    }

    #[test]
    fn a_switched_plain_profile_gets_its_own_embedded_previous_notifier() {
        let profile_a = plan_notify_integration(
            Some(vec!["notifier-a".to_owned()]),
            &managed(),
            &programs(),
            None,
        )
        .expect("profile A");
        let profile_b = plan_notify_integration(
            Some(vec!["notifier-b".to_owned()]),
            &managed(),
            &programs(),
            Some(
                profile_a
                    .previous_notify
                    .as_deref()
                    .expect("profile A previous"),
            ),
        )
        .expect("profile B");

        assert_eq!(
            embedded_previous(&profile_a.managed_command, FORWARD_NOTIFY_FLAG),
            Some(vec!["notifier-a".to_owned()])
        );
        assert_eq!(
            embedded_previous(&profile_b.managed_command, FORWARD_NOTIFY_FLAG),
            Some(vec!["notifier-b".to_owned()])
        );
    }

    #[test]
    fn malformed_computer_use_previous_json_is_rejected() {
        let active = vec![
            "/tmp/SkyComputerUseClient".to_owned(),
            "turn-ended".to_owned(),
            "--previous-notify".to_owned(),
            "not-json".to_owned(),
        ];
        let error = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect_err("malformed wrapper must fail");
        assert!(error.to_string().contains("JSON string array"));
    }

    #[test]
    fn an_unknown_computer_use_executable_is_not_treated_as_a_normal_notifier() {
        let active = vec![
            "/tmp/SkyComputerUseClientV2".to_owned(),
            "turn-ended".to_owned(),
        ];
        let error = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect_err("unknown Computer Use format must fail closed");
        assert!(error.to_string().contains("unsupported Computer Use"));
    }

    #[test]
    fn foreign_codex_notify_is_not_nested() {
        let active = vec![
            "/different/codex-notify".to_owned(),
            "notify".to_owned(),
            MANAGED_NOTIFY_FLAG.to_owned(),
        ];
        let error = plan_notify_integration(Some(active), &managed(), &programs(), None)
            .expect_err("foreign installation must fail");
        assert!(error.to_string().contains("another codex-notify"));
    }
}
