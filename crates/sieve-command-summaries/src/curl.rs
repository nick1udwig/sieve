use crate::{
    is_curl_command, is_short_flag_cluster, known_outcome, split_flag_value, unknown_outcome,
    unknown_with_flags, SummaryOutcome,
};
use sieve_policy::{canonicalize_net_origin_scope, canonicalize_sink_key};
use sieve_types::{Action, Capability, CommandSummary, Resource, SinkCheck, SinkKey, ValueRef};

pub(crate) fn summarize_curl(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_curl_command(argv) {
        return None;
    }

    #[derive(Debug, Clone)]
    struct PayloadArg {
        flag: String,
        value_index: usize,
    }

    let mut method: Option<String> = None;
    let mut url_raw: Option<String> = None;
    let mut payloads: Vec<PayloadArg> = Vec::new();
    let mut unsupported_flags: Vec<String> = Vec::new();
    let mut i = 1usize;
    let mut saw_end_of_flags = false;

    while i < argv.len() {
        let arg = &argv[i];
        if !saw_end_of_flags && arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }

        if !saw_end_of_flags && arg.starts_with('-') {
            if arg == "-X" || arg == "--request" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl method flag missing value"));
                }
                method = Some(argv[i + 1].to_ascii_uppercase());
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--request=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl method flag missing value"));
                }
                method = Some(raw.to_ascii_uppercase());
                i += 1;
                continue;
            }

            if arg == "--url" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl url flag missing value"));
                }
                url_raw = Some(argv[i + 1].clone());
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--url=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl url flag missing value"));
                }
                url_raw = Some(raw.to_string());
                i += 1;
                continue;
            }

            if arg == "-d"
                || arg == "--data"
                || arg == "--data-raw"
                || arg == "--data-binary"
                || arg == "--data-ascii"
                || arg == "--data-urlencode"
                || arg == "--json"
            {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl payload flag missing value"));
                }
                payloads.push(PayloadArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                });
                i += 2;
                continue;
            }

            if arg.starts_with("-d") && arg.len() > 2 {
                payloads.push(PayloadArg {
                    flag: "-d".to_string(),
                    value_index: i,
                });
                i += 1;
                continue;
            }

            if let Some((flag, _value)) = split_flag_value(arg) {
                if matches!(
                    flag,
                    "--data"
                        | "--data-raw"
                        | "--data-binary"
                        | "--data-ascii"
                        | "--data-urlencode"
                        | "--json"
                ) {
                    payloads.push(PayloadArg {
                        flag: flag.to_string(),
                        value_index: i,
                    });
                    i += 1;
                    continue;
                }
            }

            if matches!(
                arg.as_str(),
                "-s" | "-S"
                    | "-L"
                    | "-k"
                    | "-f"
                    | "--silent"
                    | "--show-error"
                    | "--location"
                    | "--insecure"
                    | "--fail"
                    | "--fail-with-body"
            ) {
                i += 1;
                continue;
            }

            if is_short_flag_cluster(arg, &['s', 'S', 'L', 'k', 'f']) {
                i += 1;
                continue;
            }

            if arg == "-H" || arg == "--header" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl header flag missing value"));
                }
                i += 2;
                continue;
            }

            if arg.starts_with("--header=") {
                i += 1;
                continue;
            }

            unsupported_flags.push(arg.clone());
            i += 1;
            continue;
        }

        if url_raw.is_none() {
            url_raw = Some(arg.clone());
        }
        i += 1;
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported curl flags",
            unsupported_flags,
        ));
    }

    let method = method.unwrap_or_else(|| {
        if payloads.is_empty() {
            "GET".to_string()
        } else {
            "POST".to_string()
        }
    });
    if matches!(method.as_str(), "GET" | "HEAD") {
        let Some(url) = url_raw else {
            return Some(unknown_outcome("curl request missing URL"));
        };
        let Some(sink) = canonicalize_url_connect_scope(&url) else {
            return Some(unknown_outcome("curl request has invalid URL sink"));
        };
        return Some(known_outcome(CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: sink.0.clone(),
            }],
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        }));
    }
    if !matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return None;
    }

    let Some(url) = url_raw else {
        return Some(unknown_outcome("curl mutating request missing URL"));
    };
    let Some(sink) = canonicalize_url_sink(&url) else {
        return Some(unknown_outcome(
            "curl mutating request has invalid URL sink",
        ));
    };

    let sink_checks = payloads
        .into_iter()
        .map(|payload| SinkCheck {
            argument_name: payload.flag,
            sink: sink.clone(),
            value_refs: vec![ValueRef(format!("argv:{}", payload.value_index))],
        })
        .collect();

    Some(known_outcome(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: sink.0.clone(),
        }],
        sink_checks,
        unsupported_flags: Vec::new(),
    }))
}

pub(crate) fn canonicalize_url_connect_scope(raw: &str) -> Option<SinkKey> {
    canonicalize_net_origin_scope(raw).map(|origin| SinkKey(format!("{origin}/")))
}

fn canonicalize_url_sink(raw: &str) -> Option<SinkKey> {
    canonicalize_sink_key(raw).ok().map(SinkKey)
}
