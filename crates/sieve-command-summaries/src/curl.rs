use crate::{
    is_curl_command, is_short_flag_cluster, known_summary_outcome, split_flag_value,
    unknown_outcome, unknown_with_flags, SummaryOutcome,
};
use sieve_policy::{canonicalize_net_origin_scope, canonicalize_sink_key};
use sieve_types::{Action, Capability, Resource, SinkCheck, SinkKey, ValueRef};

#[derive(Debug, Clone)]
struct SinkArg {
    flag: String,
    value_index: usize,
    value: String,
}

const PAYLOAD_VALUE_FLAGS: &[&str] = &[
    "-d",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-ascii",
    "--data-urlencode",
    "--json",
];

const LONG_PAYLOAD_VALUE_FLAGS: &[&str] = &[
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-ascii",
    "--data-urlencode",
    "--json",
];

const IGNORED_NO_VALUE_FLAGS: &[&str] = &[
    "-s",
    "-S",
    "-L",
    "-k",
    "-f",
    "--silent",
    "--show-error",
    "--location",
    "--insecure",
    "--fail",
    "--fail-with-body",
    "--compressed",
    "--http1.1",
    "--http2",
    "--ipv4",
    "--ipv6",
    "--no-progress-meter",
];

pub(crate) fn summarize_curl(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_curl_command(argv) {
        return None;
    }

    let mut method: Option<String> = None;
    let mut url_raw: Option<String> = None;
    let mut payloads: Vec<SinkArg> = Vec::new();
    let mut request_values: Vec<SinkArg> = Vec::new();
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

            if let Some(raw) = attached_short_value(arg, "-X") {
                method = Some(raw.to_ascii_uppercase());
                i += 1;
                continue;
            }

            if matches!(arg.as_str(), "-I" | "--head") {
                method = Some("HEAD".to_string());
                i += 1;
                continue;
            }

            if matches!(arg.as_str(), "-G" | "--get") {
                method = Some("GET".to_string());
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

            if PAYLOAD_VALUE_FLAGS.contains(&arg.as_str()) {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl payload flag missing value"));
                }
                let value = argv[i + 1].clone();
                payloads.push(SinkArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                    value,
                });
                i += 2;
                continue;
            }

            if arg.starts_with("-d") && arg.len() > 2 {
                payloads.push(SinkArg {
                    flag: "-d".to_string(),
                    value_index: i,
                    value: arg[2..].to_string(),
                });
                i += 1;
                continue;
            }

            if let Some((flag, value)) = split_flag_value(arg) {
                if LONG_PAYLOAD_VALUE_FLAGS.contains(&flag) {
                    payloads.push(SinkArg {
                        flag: flag.to_string(),
                        value_index: i,
                        value: value.to_string(),
                    });
                    i += 1;
                    continue;
                }
            }

            if IGNORED_NO_VALUE_FLAGS.contains(&arg.as_str()) {
                i += 1;
                continue;
            }

            if is_short_flag_cluster(arg, &['s', 'S', 'L', 'k', 'f', '4', '6']) {
                i += 1;
                continue;
            }

            if arg == "-H" || arg == "--header" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl header flag missing value"));
                }
                let value = argv[i + 1].clone();
                request_values.push(SinkArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                    value,
                });
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--header=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl header flag missing value"));
                }
                request_values.push(SinkArg {
                    flag: "--header".to_string(),
                    value_index: i,
                    value: raw.to_string(),
                });
                i += 1;
                continue;
            }

            if let Some(raw) = attached_short_value(arg, "-H") {
                request_values.push(SinkArg {
                    flag: "-H".to_string(),
                    value_index: i,
                    value: raw.to_string(),
                });
                i += 1;
                continue;
            }

            if arg == "-u" || arg == "--user" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl user flag missing value"));
                }
                let value = argv[i + 1].clone();
                request_values.push(SinkArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                    value,
                });
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--user=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl user flag missing value"));
                }
                request_values.push(SinkArg {
                    flag: "--user".to_string(),
                    value_index: i,
                    value: raw.to_string(),
                });
                i += 1;
                continue;
            }

            if let Some(raw) = attached_short_value(arg, "-u") {
                request_values.push(SinkArg {
                    flag: "-u".to_string(),
                    value_index: i,
                    value: raw.to_string(),
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
        let file_read_caps = file_read_capabilities(&request_values);
        let sink_checks = if request_values.is_empty() {
            Vec::new()
        } else {
            let Some(header_sink) = canonicalize_url_sink(&url) else {
                return Some(unknown_outcome("curl request has invalid URL sink"));
            };
            sink_checks_for_args(request_values, header_sink)
        };
        let mut required_capabilities = vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: sink.0.clone(),
        }];
        required_capabilities.extend(file_read_caps);
        return Some(known_summary_outcome(required_capabilities, sink_checks));
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

    request_values.extend(payloads);
    let mut required_capabilities = vec![Capability {
        resource: Resource::Net,
        action: Action::Write,
        scope: sink.0.clone(),
    }];
    required_capabilities.extend(file_read_capabilities(&request_values));
    let sink_checks = sink_checks_for_args(request_values, sink.clone());

    Some(known_summary_outcome(required_capabilities, sink_checks))
}

pub(crate) fn canonicalize_url_connect_scope(raw: &str) -> Option<SinkKey> {
    canonicalize_net_origin_scope(raw).map(|origin| SinkKey(format!("{origin}/")))
}

fn canonicalize_url_sink(raw: &str) -> Option<SinkKey> {
    canonicalize_sink_key(raw).ok().map(SinkKey)
}

fn attached_short_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag).filter(|raw| !raw.is_empty())
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

fn file_read_capabilities(args: &[SinkArg]) -> Vec<Capability> {
    args.iter()
        .filter_map(file_read_scope)
        .map(|scope| Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope,
        })
        .collect()
}

fn file_read_scope(arg: &SinkArg) -> Option<String> {
    let path = match arg.flag.as_str() {
        "-H" | "--header" | "-d" | "--data" | "--data-ascii" | "--data-binary" | "--json" => {
            arg.value.strip_prefix('@')
        }
        "--data-urlencode" => {
            if let Some(path) = arg.value.strip_prefix('@') {
                Some(path)
            } else if let Some((name, path)) = arg.value.split_once('@') {
                (!name.contains('=')).then_some(path)
            } else {
                None
            }
        }
        _ => None,
    }?;
    (!path.is_empty()).then(|| path.to_string())
}
