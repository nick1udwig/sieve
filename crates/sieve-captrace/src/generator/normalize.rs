use super::templates::{contains_template_token, is_template_token, looks_like_url};
use super::types::GeneratedSummaryOutcome;
use crate::fixture::{FixtureLayout, TOKEN_URL};
use sieve_command_summaries::SummaryOutcome as ExistingSummaryOutcome;
use sieve_types::{Capability, CommandKnowledge, CommandSummary, Resource};
use std::collections::BTreeSet;

pub fn derive_summary_from_trace(
    attempted: &[Capability],
    fixture: &FixtureLayout,
) -> CommandSummary {
    let mut deduped = BTreeSet::new();
    let mut required_capabilities = Vec::new();

    for capability in attempted {
        if !should_keep_capability(capability, fixture) {
            continue;
        }
        let mut normalized = capability.clone();
        normalized.scope = normalize_capability_scope_for_definition(&normalized, fixture);
        let key = capability_key(&normalized);
        if deduped.insert(key) {
            required_capabilities.push(normalized);
        }
    }

    CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    }
}

pub(super) fn normalize_existing_summary_outcome_for_definition(
    mut outcome: ExistingSummaryOutcome,
    fixture: &FixtureLayout,
    literal_replacements: &[(String, String)],
) -> ExistingSummaryOutcome {
    if let Some(summary) = outcome.summary.as_mut() {
        normalize_command_summary_for_definition(summary, fixture, literal_replacements);
    }
    outcome
}

pub(super) fn build_literal_template_replacements(
    raw_argv_template: &[String],
    argv_template: &[String],
) -> Vec<(String, String)> {
    let mut replacements = Vec::new();
    let mut seen = BTreeSet::new();
    for (raw, template) in raw_argv_template.iter().zip(argv_template.iter()) {
        if raw == template || raw.is_empty() || !contains_template_token(template) {
            continue;
        }
        if seen.insert(raw.clone()) {
            replacements.push((raw.clone(), template.clone()));
        }
    }
    replacements.sort_by(|lhs, rhs| {
        rhs.0
            .len()
            .cmp(&lhs.0.len())
            .then_with(|| lhs.0.cmp(&rhs.0))
    });
    replacements
}

pub(super) fn choose_summary_outcome(
    existing: ExistingSummaryOutcome,
    trace_derived: &CommandSummary,
) -> (GeneratedSummaryOutcome, Option<bool>) {
    if existing.knowledge == CommandKnowledge::Known {
        let matches = existing.summary.as_ref() == Some(trace_derived);
        let reason = if matches {
            "matched existing command summary".to_string()
        } else {
            "used existing command summary (trace differed)".to_string()
        };
        return (
            GeneratedSummaryOutcome {
                knowledge: existing.knowledge,
                summary: existing.summary,
                reason: Some(reason),
            },
            Some(matches),
        );
    }

    if let Some(summary) = existing.summary {
        return (
            GeneratedSummaryOutcome {
                knowledge: existing.knowledge,
                summary: Some(summary),
                reason: existing.reason,
            },
            None,
        );
    }

    (
        GeneratedSummaryOutcome {
            knowledge: CommandKnowledge::Unknown,
            summary: Some(trace_derived.clone()),
            reason: Some("trace-derived candidate; no existing summary".to_string()),
        },
        None,
    )
}

fn normalize_command_summary_for_definition(
    summary: &mut CommandSummary,
    fixture: &FixtureLayout,
    literal_replacements: &[(String, String)],
) {
    for capability in &mut summary.required_capabilities {
        let normalized_scope = normalize_capability_scope_for_definition(capability, fixture);
        capability.scope =
            apply_literal_template_replacements(&normalized_scope, literal_replacements);
    }
    dedupe_capabilities(&mut summary.required_capabilities);

    for check in &mut summary.sink_checks {
        check.sink.0 = normalize_sink_scope_for_definition(&check.sink.0, literal_replacements);
    }
}

fn dedupe_capabilities(capabilities: &mut Vec<Capability>) {
    let mut seen = BTreeSet::new();
    capabilities.retain(|capability| seen.insert(capability_key(capability)));
}

fn normalize_sink_scope_for_definition(
    scope: &str,
    literal_replacements: &[(String, String)],
) -> String {
    let replaced = apply_literal_template_replacements(scope, literal_replacements);
    if is_template_token(&replaced) || contains_template_token(&replaced) {
        return replaced;
    }
    if looks_like_url(&replaced) {
        return TOKEN_URL.to_string();
    }
    replaced
}

fn apply_literal_template_replacements(
    value: &str,
    literal_replacements: &[(String, String)],
) -> String {
    let mut out = value.to_string();
    for (literal, template) in literal_replacements {
        if literal.is_empty() || literal == template {
            continue;
        }
        out = out.replace(literal, template);
    }
    out
}

fn normalize_capability_scope_for_definition(
    capability: &Capability,
    fixture: &FixtureLayout,
) -> String {
    match capability.resource {
        Resource::Fs => fixture.normalize_scope_for_definition(&capability.scope),
        Resource::Net => normalize_network_scope_for_definition(&capability.scope),
        Resource::Ipc if capability.action == sieve_types::Action::Connect => {
            "ipc=local".to_string()
        }
        _ => capability.scope.clone(),
    }
}

fn normalize_network_scope_for_definition(scope: &str) -> String {
    if scope.starts_with("network=") {
        return scope.to_string();
    }

    if let Some(bucket) = network_scope_bucket(scope) {
        return format!("network={bucket}");
    }

    if looks_like_url(scope) {
        return "network=remote".to_string();
    }

    scope.to_string()
}

fn network_scope_bucket(scope: &str) -> Option<&'static str> {
    if let Some(address) = scope_field(scope, "address=") {
        return Some(if network_address_is_local(address) {
            "local"
        } else {
            "remote"
        });
    }

    let host = url_host(scope)?;
    Some(if network_address_is_local(host) {
        "local"
    } else {
        "remote"
    })
}

fn url_host(scope: &str) -> Option<&str> {
    let rest = scope
        .strip_prefix("http://")
        .or_else(|| scope.strip_prefix("https://"))?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if let Some(bracketed) = host_port.strip_prefix('[') {
        let end = bracketed.find(']')?;
        return Some(&bracketed[..end]);
    }
    if let Some((host, _port)) = host_port.split_once(':') {
        if host.is_empty() {
            return None;
        }
        return Some(host);
    }
    Some(host_port)
}

fn network_address_is_local(address: &str) -> bool {
    let lowered = address.to_ascii_lowercase();
    if matches!(address, "127.0.0.1" | "::1" | "localhost" | "127.0.0.53") {
        return true;
    }
    if lowered == "localhost" || lowered.ends_with(".localhost") {
        return true;
    }
    if address.starts_with("127.") || address.starts_with("10.") || address.starts_with("192.168.")
    {
        return true;
    }
    if let Some(rest) = address.strip_prefix("172.") {
        if let Some(second_octet) = rest.split('.').next() {
            if let Ok(value) = second_octet.parse::<u8>() {
                if (16..=31).contains(&value) {
                    return true;
                }
            }
        }
    }
    if address.starts_with("169.254.") {
        return true;
    }
    lowered == "::1"
        || lowered.starts_with("fc")
        || lowered.starts_with("fd")
        || lowered.starts_with("fe80")
}

fn scope_field<'a>(scope: &'a str, key: &str) -> Option<&'a str> {
    let start = scope.find(key)? + key.len();
    let tail = &scope[start..];
    let end = tail.find(',').unwrap_or(tail.len());
    Some(tail[..end].trim())
}

fn should_keep_capability(capability: &Capability, fixture: &FixtureLayout) -> bool {
    match capability.resource {
        Resource::Fs => {
            let root = fixture.root.to_string_lossy();
            capability.scope == root || capability.scope.starts_with(root.as_ref())
        }
        Resource::Net | Resource::Ipc => true,
        Resource::Proc | Resource::Env => false,
    }
}

fn capability_key(capability: &Capability) -> String {
    format!(
        "{:?}\u{1f}{:?}\u{1f}{}",
        capability.resource, capability.action, capability.scope
    )
}
