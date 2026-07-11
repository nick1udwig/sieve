use std::fs;
use std::path::Path;

#[test]
fn telegram_runtime_does_not_require_curl_subprocesses() {
    for path in [
        "src/media.rs",
        "../sieve-interface-telegram/src/transport.rs",
    ] {
        let body = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
            .expect("read Telegram source");
        assert!(!body.contains("Command::new(\"curl\")"), "{path}");
        assert!(!body.contains("TokioCommand::new(\"curl\")"), "{path}");
    }
}

#[test]
fn sieve_app_manifest_uses_remote_sieve_lcm_dependency() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read sieve-app Cargo.toml");
    let dependency_line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("sieve-lcm = "))
        .expect("sieve-lcm dependency line");

    assert!(
        dependency_line.contains("git = \"https://github.com/nick1udwig/sieve-lcm\""),
        "expected sieve-lcm to resolve from the public git repo"
    );
    assert!(
        !dependency_line.contains("path = "),
        "sieve-lcm must not depend on a sibling checkout path"
    );
}
