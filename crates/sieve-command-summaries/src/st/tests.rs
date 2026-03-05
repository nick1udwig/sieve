use super::*;
use sieve_types::{Action, Capability, CommandKnowledge, Resource};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn st_stt_audio_file_is_known_with_read_and_connect() {
    let out = summarize_st(&argv(&["st", "stt", "/tmp/input.ogg"])).expect("st summary");
    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/input.ogg".to_string(),
            },
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: OPENAI_CONNECT_SCOPE.to_string(),
            }
        ]
    );
}

#[test]
fn st_stt_output_path_adds_fs_write() {
    let out = summarize_st(&argv(&[
        "st",
        "stt",
        "/tmp/input.ogg",
        "--output",
        "/tmp/out.txt",
    ]))
    .expect("st summary");
    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("summary");
    assert_eq!(
        summary.required_capabilities[2],
        Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp/out.txt".to_string(),
        }
    );
}

#[test]
fn st_tts_text_file_and_output_is_known() {
    let out = summarize_st(&argv(&[
        "st",
        "tts",
        "/tmp/input.txt",
        "--format",
        "ogg",
        "--output",
        "/tmp/out.ogg",
    ]))
    .expect("st summary");
    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: OPENAI_CONNECT_SCOPE.to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/input.txt".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/out.ogg".to_string(),
            }
        ]
    );
}

#[test]
fn st_tts_txt_input_is_known_without_fs_read() {
    let out = summarize_st(&argv(&[
        "st",
        "tts",
        "--txt",
        "hello",
        "--output",
        "/tmp/out.ogg",
    ]))
    .expect("st summary");
    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: OPENAI_CONNECT_SCOPE.to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/out.ogg".to_string(),
            }
        ]
    );
}

#[test]
fn st_tts_missing_input_is_unknown() {
    let out = summarize_st(&argv(&["st", "tts", "--output", "/tmp/out.ogg"])).expect("st summary");
    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(out.reason.as_deref(), Some("st tts missing text input"));
}
