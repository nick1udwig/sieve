use crate::{
    is_curl_command, is_short_flag_cluster, known_outcome, split_flag_value, unknown_outcome,
    unknown_with_flags, SummaryOutcome,
};
use sieve_policy::{canonicalize_net_origin_scope, canonicalize_sink_key};
use sieve_types::{Action, Capability, CommandSummary, Resource, SinkCheck, SinkKey, ValueRef};

#[derive(Debug, Clone)]
struct SinkArg {
    flag: String,
    value_index: usize,
}

pub(crate) fn summarize_curl(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_curl_command(argv) {
        return None;
    }

    let mut method: Option<String> = None;
    let mut url_raw: Option<String> = None;
    let mut payloads: Vec<SinkArg> = Vec::new();
    let mut headers: Vec<SinkArg> = Vec::new();
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
                payloads.push(SinkArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                });
                i += 2;
                continue;
            }

            if arg.starts_with("-d") && arg.len() > 2 {
                payloads.push(SinkArg {
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
                    payloads.push(SinkArg {
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
                headers.push(SinkArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                });
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--header=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl header flag missing value"));
                }
                headers.push(SinkArg {
                    flag: "--header".to_string(),
                    value_index: i,
                });
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
    let has_payloads = !payloads.is_empty();
    let method = method.as_str();
    let is_read_only_method = matches!(method, "GET" | "HEAD");
    if is_read_only_method && !has_payloads {
        let Some(url) = url_raw else {
            return Some(unknown_outcome("curl request missing URL"));
        };
        let Some(sink) = canonicalize_url_connect_scope(&url) else {
            return Some(unknown_outcome("curl request has invalid URL sink"));
        };
        let sink_checks = if headers.is_empty() {
            Vec::new()
        } else {
            let Some(header_sink) = canonicalize_url_sink(&url) else {
                return Some(unknown_outcome("curl request has invalid URL sink"));
            };
            sink_checks_for_args(headers, header_sink)
        };
        return Some(known_outcome(CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: sink.0.clone(),
            }],
            sink_checks,
            unsupported_flags: Vec::new(),
        }));
    }
    let is_write_method = matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
        || (is_read_only_method && has_payloads);
    if !is_write_method {
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

    headers.extend(payloads);
    let sink_checks = sink_checks_for_args(headers, sink.clone());

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

fn sink_checks_for_args(args: Vec<SinkArg>, sink: SinkKey) -> Vec<SinkCheck> {
    args.into_iter()
        .map(|arg| SinkCheck {
            argument_name: arg.flag,
            sink: sink.clone(),
            value_refs: vec![ValueRef(format!("argv:{}", arg.value_index))],
        })
        .collect()
}
