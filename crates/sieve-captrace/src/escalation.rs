use crate::cli::{CliArgs, CliNetworkMode};
use sieve_captrace::GeneratedCommandDefinition;
use sieve_types::{Action, Resource};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct EscalationHint {
    pub(crate) arg: String,
    pub(crate) reason: String,
}

pub(crate) fn collect_escalation_hints(
    definition: &GeneratedCommandDefinition,
    args: &CliArgs,
) -> Vec<EscalationHint> {
    let mut hints = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    let saw_net_attempt = definition.variants.iter().any(|variant| {
        variant
            .attempted_capabilities
            .iter()
            .any(|cap| cap.resource == Resource::Net)
    });
    let saw_unshare_net_failure = definition.variants.iter().any(|variant| {
        variant
            .trace_error
            .as_deref()
            .is_some_and(|err| err.contains("NETLINK_ROUTE socket"))
    });

    if args.network_mode != CliNetworkMode::Full && (saw_net_attempt || saw_unshare_net_failure) {
        if args.network_mode == CliNetworkMode::Isolated {
            push_hint(
                &mut hints,
                &mut seen,
                "--allow-local-network".to_string(),
                "allow loopback-only networking in sandbox".to_string(),
            );
        }
        push_hint(
            &mut hints,
            &mut seen,
            "--allow-full-network".to_string(),
            "allow outbound networking in sandbox".to_string(),
        );
    }

    let blocked_write_scopes = collect_blocked_write_scopes(definition, &args.allow_write_paths);
    if let Some(example) = blocked_write_scopes.first() {
        push_hint(
            &mut hints,
            &mut seen,
            format!("--allow-write {example}"),
            "permit observed filesystem writes outside default writable roots".to_string(),
        );
    }

    hints
}

fn collect_blocked_write_scopes(
    definition: &GeneratedCommandDefinition,
    allow_write_paths: &[PathBuf],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for variant in &definition.variants {
        for capability in &variant.attempted_capabilities {
            if capability.resource != Resource::Fs {
                continue;
            }
            if capability.action != Action::Write && capability.action != Action::Append {
                continue;
            }
            let scope = capability.scope.as_str();
            if scope.is_empty() || scope.starts_with("{{") || scope == "/" {
                continue;
            }
            if is_default_writable_scope(scope) || is_allowed_by_user(scope, allow_write_paths) {
                continue;
            }
            if let Some(example) = writable_hint_path(scope) {
                if seen.insert(example.clone()) {
                    out.push(example);
                }
            }
        }
    }

    out
}

fn is_default_writable_scope(scope: &str) -> bool {
    scope == "/tmp" || scope.starts_with("/tmp/")
}

fn is_allowed_by_user(scope: &str, allow_write_paths: &[PathBuf]) -> bool {
    allow_write_paths.iter().any(|allowed| {
        let allowed = allowed.as_os_str().to_string_lossy();
        scope == allowed || scope.starts_with(&format!("{allowed}/"))
    })
}

fn writable_hint_path(scope: &str) -> Option<String> {
    let path = Path::new(scope);
    if !path.is_absolute() {
        return None;
    }
    let candidate = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    Some(candidate.to_string_lossy().to_string())
}

fn push_hint(
    hints: &mut Vec<EscalationHint>,
    seen: &mut std::collections::BTreeSet<String>,
    arg: String,
    reason: String,
) {
    if seen.insert(arg.clone()) {
        hints.push(EscalationHint { arg, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_captrace::{GeneratedSummaryOutcome, GeneratedVariantDefinition};
    use sieve_types::{Capability, CommandKnowledge};

    #[test]
    fn escalation_hints_suggest_network_flags_when_net_attempt_seen() {
        let definition = GeneratedCommandDefinition {
            schema_version: 1,
            command: "curl".to_string(),
            generated_at_ms: 0,
            variants: vec![variant_with_caps(vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "family=af_inet,address=1.1.1.1,port=443".to_string(),
            }])],
            subcommand_reports: Vec::new(),
            notes: Vec::new(),
            rust_snippet: String::new(),
        };
        let args = cli_args(CliNetworkMode::Isolated, Vec::new());
        let hints = collect_escalation_hints(&definition, &args);
        let args_only: Vec<String> = hints.into_iter().map(|hint| hint.arg).collect();
        assert!(args_only.contains(&"--allow-local-network".to_string()));
        assert!(args_only.contains(&"--allow-full-network".to_string()));
    }

    #[test]
    fn escalation_hints_suggest_allow_write_for_blocked_write_scope() {
        let definition = GeneratedCommandDefinition {
            schema_version: 1,
            command: "tool".to_string(),
            generated_at_ms: 0,
            variants: vec![variant_with_caps(vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/root/.cache/tool/cache.db".to_string(),
            }])],
            subcommand_reports: Vec::new(),
            notes: Vec::new(),
            rust_snippet: String::new(),
        };
        let args = cli_args(CliNetworkMode::Full, Vec::new());
        let hints = collect_escalation_hints(&definition, &args);
        let args_only: Vec<String> = hints.into_iter().map(|hint| hint.arg).collect();
        assert!(args_only.contains(&"--allow-write /root/.cache/tool".to_string()));
    }

    #[test]
    fn escalation_hints_skip_allow_write_when_scope_already_allowed() {
        let definition = GeneratedCommandDefinition {
            schema_version: 1,
            command: "tool".to_string(),
            generated_at_ms: 0,
            variants: vec![variant_with_caps(vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/root/.cache/tool/cache.db".to_string(),
            }])],
            subcommand_reports: Vec::new(),
            notes: Vec::new(),
            rust_snippet: String::new(),
        };
        let args = cli_args(
            CliNetworkMode::Full,
            vec![PathBuf::from("/root/.cache/tool")],
        );
        let hints = collect_escalation_hints(&definition, &args);
        assert!(hints
            .into_iter()
            .all(|hint| !hint.arg.starts_with("--allow-write")));
    }

    fn cli_args(network_mode: CliNetworkMode, allow_write_paths: Vec<PathBuf>) -> CliArgs {
        CliArgs {
            command: "tool".to_string(),
            seed_shell_cases: Vec::new(),
            include_llm_cases: false,
            max_llm_cases: 1,
            output_path: None,
            rust_output_path: None,
            logs_root: PathBuf::from("/tmp"),
            network_mode,
            allow_write_paths,
        }
    }

    fn variant_with_caps(caps: Vec<Capability>) -> GeneratedVariantDefinition {
        GeneratedVariantDefinition {
            case_id: "case".to_string(),
            command_path: Vec::new(),
            argv_template: vec!["tool".to_string()],
            argv_effective: vec!["tool".to_string()],
            trace_path: None,
            exit_code: Some(1),
            attempted_capabilities: caps,
            trace_derived_summary: None,
            summary_outcome: GeneratedSummaryOutcome {
                knowledge: CommandKnowledge::Unknown,
                summary: None,
                reason: None,
            },
            matches_existing_summary: None,
            trace_error: None,
        }
    }
}
