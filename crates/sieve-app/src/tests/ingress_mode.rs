use super::*;

#[test]
fn run_prompt_mode_skips_stdin_and_telegram_ingress() {
    assert_eq!(
        ingress_mode_for_command(&CliCommand::Run {
            prompt: "review workspace status".to_string(),
        }),
        IngressMode {
            spawn_stdin_loop: false,
            spawn_telegram_loop: false,
        }
    );
}

#[test]
fn agent_mode_keeps_stdin_and_telegram_ingress() {
    assert_eq!(
        ingress_mode_for_command(&CliCommand::Agent),
        IngressMode {
            spawn_stdin_loop: true,
            spawn_telegram_loop: true,
        }
    );
}
