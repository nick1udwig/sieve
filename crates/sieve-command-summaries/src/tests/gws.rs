use super::argv;
use crate::{planner_command_catalog, summarize_argv};
use sieve_types::{
    Action, Capability, CommandKnowledge, Resource, SinkChannel, SinkCheck, SinkKey, ValueRef,
};

#[test]
fn planner_command_catalog_includes_gws_entry() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "gws")
        .expect("gws catalog entry");
    assert!(entry.description.contains("gws schema"));
    assert!(entry.description.contains("--dry-run"));
}

#[test]
fn gws_schema_requires_google_api_connect() {
    let out = summarize_argv(&argv(&["gws", "schema", "drive.files.list"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://www.googleapis.com/".to_string(),
        }]
    );
    assert!(summary.sink_checks.is_empty());
}

#[test]
fn gws_read_method_with_params_requires_connect_and_sink_check() {
    let out = summarize_argv(&argv(&[
        "gws",
        "drive",
        "files",
        "list",
        "--params",
        "{\"pageSize\":10}",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://www.googleapis.com/".to_string(),
        }]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "--params".to_string(),
            sink: SinkKey("https://www.googleapis.com/".to_string()),
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef("argv:5".to_string())],
        }]
    );
}

#[test]
fn gws_write_method_with_json_and_output_maps_caps() {
    let out = summarize_argv(&argv(&[
        "gws",
        "sheets",
        "spreadsheets",
        "values",
        "append",
        "--params",
        "{\"spreadsheetId\":\"id\"}",
        "--json",
        "{\"values\":[[\"A\"]]}",
        "--output",
        "result.json",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://www.googleapis.com/".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "result.json".to_string(),
            }
        ]
    );
    assert_eq!(
        summary.sink_checks,
        vec![
            SinkCheck {
                argument_name: "--params".to_string(),
                sink: SinkKey("https://www.googleapis.com/".to_string()),
                channel: SinkChannel::Body,
                value_refs: vec![ValueRef("argv:6".to_string())],
            },
            SinkCheck {
                argument_name: "--json".to_string(),
                sink: SinkKey("https://www.googleapis.com/".to_string()),
                channel: SinkChannel::Body,
                value_refs: vec![ValueRef("argv:8".to_string())],
            }
        ]
    );
}

#[test]
fn gws_upload_uses_upload_origin_and_reads_local_file() {
    let out = summarize_argv(&argv(&[
        "gws",
        "drive",
        "files",
        "create",
        "--json",
        "{\"name\":\"report.pdf\"}",
        "--upload",
        "/tmp/report.pdf",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://www.googleapis.com/upload/".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/report.pdf".to_string(),
            }
        ]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "--json".to_string(),
            sink: SinkKey("https://www.googleapis.com/upload/".to_string()),
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef("argv:5".to_string())],
        }]
    );
}

#[test]
fn gws_sanitize_adds_modelarmor_write_capability() {
    let out = summarize_argv(&argv(&[
        "gws",
        "drive",
        "files",
        "list",
        "--sanitize",
        "projects/p/locations/l/templates/t",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://www.googleapis.com/".to_string(),
            },
            Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://modelarmor.googleapis.com/".to_string(),
            }
        ]
    );
}

#[test]
fn gws_dry_run_is_known_noop() {
    let out = summarize_argv(&argv(&[
        "gws",
        "drive",
        "files",
        "create",
        "--json",
        "{\"name\":\"demo\"}",
        "--output",
        "ignored.json",
        "--dry-run",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert!(summary.required_capabilities.is_empty());
    assert!(summary.sink_checks.is_empty());
}

#[test]
fn gws_service_helper_routes_to_unknown() {
    let out = summarize_argv(&argv(&["gws", "drive", "+upload", "--file", "report.pdf"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some("gws service helpers are unsupported")
    );
}

#[test]
fn gws_auth_export_routes_to_unknown() {
    let out = summarize_argv(&argv(&["gws", "auth", "export"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some("gws auth commands are unsupported")
    );
}

#[test]
fn gws_unsupported_flag_routes_to_unknown() {
    let out = summarize_argv(&argv(&["gws", "drive", "files", "list", "--resolve-refs"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.unsupported_flags,
        vec!["--resolve-refs".to_string()]
    );
}
