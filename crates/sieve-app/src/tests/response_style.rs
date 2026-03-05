use super::*;
#[test]
fn enforce_link_policy_appends_plain_urls_when_link_claim_has_no_url() {
    let message = "For more information, visit the provided link.".to_string();
    let enforced = enforce_link_policy(
        message,
        &[
            "https://example.com/a".to_string(),
            "https://example.com/b".to_string(),
        ],
        "please include sources and links",
    );
    assert!(enforced.contains("https://example.com/a"));
    assert!(enforced.contains("https://example.com/b"));
    assert!(enforced.contains("provided link"));
}

#[test]
fn enforce_link_policy_strips_link_claim_without_available_urls() {
    let message = "Top result is ready. Visit the provided link for details.".to_string();
    let enforced = enforce_link_policy(message, &[], "just answer briefly");
    assert!(!enforced.to_ascii_lowercase().contains("provided link"));
}

#[test]
fn enforce_link_policy_does_not_append_urls_when_sources_not_requested() {
    let message = "For more information, visit the provided link.".to_string();
    let enforced = enforce_link_policy(
        message,
        &["https://example.com/a".to_string()],
        "just answer",
    );
    assert!(!enforced.contains("https://example.com/a"));
    assert!(!enforced.to_ascii_lowercase().contains("provided link"));
}

#[test]
fn enforce_link_policy_keeps_actionable_link_wording() {
    let message = "Yes—I can read your message here and respond. If you mean actual audio, upload an audio file (or share a link) and tell me what you want (e.g., transcription or a summary).".to_string();
    let enforced = enforce_link_policy(message.clone(), &[], "just answer briefly");
    assert_eq!(enforced, message);
}

#[test]
fn enforce_link_policy_keeps_source_origin_wording() {
    let message = "Strength through Unity, Unity through Faith is best known as a Norsefire slogan in V for Vendetta. If you tell me what you need (e.g., identify the source, quote context, or explain the reference), I can help.".to_string();
    let enforced = enforce_link_policy(message.clone(), &[], "just answer briefly");
    assert_eq!(enforced, message);
}

#[test]
fn filter_non_asset_urls_removes_asset_links() {
    let filtered = filter_non_asset_urls(vec![
            "https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125"
                .to_string(),
            "https://imgs.search.brave.com/fs6uyhM5xA6gctiAKJTHhWtpR2YRWceKfG_9aqjmfRs/rs:fit:32:32:1:0/g:ce/a.png"
                .to_string(),
            "https://example.com/favicon.ico".to_string(),
        ]);
    assert_eq!(
        filtered,
        vec![
            "https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125".to_string()
        ]
    );
}

#[test]
fn strip_asset_urls_from_message_removes_asset_plain_urls() {
    let message = "Useful: https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125\nhttps://imgs.search.brave.com/example.png";
    let stripped = strip_asset_urls_from_message(message);
    assert!(stripped
        .contains("https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125"));
    assert!(!stripped.contains("https://imgs.search.brave.com/example.png"));
}

#[test]
fn extract_plain_urls_from_text_handles_jsonish_tokens() {
    let text = "{\"url\":\"https://weather.com/weather/tenday/l/Dublin\",\"other\":\"x\"}";
    let urls = extract_plain_urls_from_text(text);
    assert_eq!(
        urls,
        vec!["https://weather.com/weather/tenday/l/Dublin".to_string()]
    );
}

#[test]
fn source_and_detail_request_detection() {
    assert!(user_requested_sources("please include links to sources"));
    assert!(user_requested_sources("cite references"));
    assert!(!user_requested_sources("just tell me the answer"));

    assert!(user_requested_detailed_output(
        "give a detailed explanation"
    ));
    assert!(user_requested_detailed_output("step by step please"));
    assert!(!user_requested_detailed_output("just short answer"));
}

#[test]
fn concise_style_diagnostic_flags_unsolicited_source_dump() {
    let message = "Answer first. https://a.example https://b.example";
    let diagnostic = concise_style_diagnostic(message, "What is the answer?");
    assert!(diagnostic.is_some());
    let detail_ok = concise_style_diagnostic(message, "Give sources and links");
    assert!(detail_ok.is_none());
}

#[test]
fn load_dotenv_from_path_missing_file_is_noop() {
    let _guard = env_test_lock()
        .lock()
        .expect("dotenv env test lock poisoned");
    let path = std::env::temp_dir().join(format!(
        "sieve-app-missing-env-{}-{}",
        std::process::id(),
        now_ms()
    ));
    assert!(load_dotenv_from_path(&path).is_ok());
}

#[test]
fn load_dotenv_from_path_sets_values() {
    let _guard = env_test_lock()
        .lock()
        .expect("dotenv env test lock poisoned");
    let tmp_dir = std::env::temp_dir().join(format!(
        "sieve-app-dotenv-test-{}-{}",
        std::process::id(),
        now_ms()
    ));
    fs::create_dir_all(&tmp_dir).expect("create temp test dir");
    let env_path = tmp_dir.join(".env");
    let key = format!("SIEVE_APP_DOTENV_TEST_{}_{}", std::process::id(), now_ms());
    std::env::remove_var(&key);
    fs::write(&env_path, format!("{key}=from_dotenv\n")).expect("write dotenv file");

    load_dotenv_from_path(&env_path).expect("load dotenv from path");
    let loaded = std::env::var(&key).expect("dotenv variable must be set");
    assert_eq!(loaded, "from_dotenv");

    std::env::remove_var(&key);
    let _ = fs::remove_file(&env_path);
    let _ = fs::remove_dir(&tmp_dir);
}
