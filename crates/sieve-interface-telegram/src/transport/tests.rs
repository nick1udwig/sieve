use super::*;
use std::collections::VecDeque;

struct TestExecutor {
    responses: VecDeque<Result<String, String>>,
    commands: Vec<(String, Vec<String>)>,
}

impl TestExecutor {
    fn new(responses: Vec<Result<String, String>>) -> Self {
        Self {
            responses: responses.into(),
            commands: Vec::new(),
        }
    }
}

impl CommandExecutor for TestExecutor {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        self.commands.push((
            program.to_string(),
            args.iter().map(|arg| arg.to_string()).collect(),
        ));
        self.responses
            .pop_front()
            .expect("missing stubbed executor response")
    }
}

#[test]
fn get_updates_uses_expected_curl_url_and_maps_updates() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":[{\"update_id\":5,\"message\":{\"message_id\":77,\"chat\":{\"id\":42},\"from\":{\"id\":1001},\"text\":\"/approve apr_1\"}}]}"
                .to_string(),
        )]),
    );

    let updates = poller
        .get_updates(Some(9), 30)
        .expect("get updates must succeed");

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].update_id, 5);
    assert_eq!(updates[0].message.as_ref().expect("msg").chat_id, 42);
    assert_eq!(
        updates[0].message.as_ref().expect("msg").sender_user_id,
        Some(1001)
    );
    assert_eq!(updates[0].message.as_ref().expect("msg").message_id, 77);
    assert!(updates[0].message_reaction.is_none());

    let commands = &poller.executor.commands;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, "curl");
    assert_eq!(commands[0].1[0], "-sS");
    assert_eq!(commands[0].1[1], "--fail");
    let url = &commands[0].1[2];
    assert!(url.contains("/bottoken_abc/getUpdates?"));
    assert!(url.contains("timeout=30"));
    assert!(url.contains("offset=9"));
    assert!(url.contains("allowed_updates=%5B%22message%22%2C%22message_reaction%22%5D"));
}

#[test]
fn get_updates_requests_message_reaction_update_type() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok("{\"ok\":true,\"result\":[]}".to_string())]),
    );

    poller
        .get_updates(Some(1), 30)
        .expect("get updates must succeed");

    let commands = &poller.executor.commands;
    assert_eq!(commands.len(), 1);
    let url = &commands[0].1[2];
    assert!(
        url.contains("allowed_updates=%5B%22message%22%2C%22message_reaction%22%5D"),
        "expected encoded allowed_updates with message_reaction in {url}"
    );
}

#[test]
fn send_message_posts_json_payload() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":{\"message_id\":123}}".to_string()
        )]),
    );

    let message_id = poller
        .send_message(42, "hi", None)
        .expect("send message must succeed");
    assert_eq!(message_id, Some(123));

    let commands = &poller.executor.commands;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, "curl");
    assert_eq!(commands[0].1[2], "-X");
    assert_eq!(commands[0].1[3], "POST");
    assert_eq!(commands[0].1[4], "-H");
    assert_eq!(commands[0].1[5], "Content-Type: application/json");
    assert_eq!(commands[0].1[6], "-d");
    assert!(commands[0].1[7].contains("\"chat_id\":42"));
    assert!(commands[0].1[8].contains("/bottoken_abc/sendMessage"));
}

#[test]
fn send_message_can_reply_to_existing_message() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":{\"message_id\":124}}".to_string()
        )]),
    );

    let _ = poller
        .send_message(42, "status reply", Some(77))
        .expect("send reply message must succeed");

    let commands = &poller.executor.commands;
    assert_eq!(commands.len(), 1);
    assert!(commands[0].1[7].contains("\"reply_parameters\":{\"message_id\":77}"));
}

#[test]
fn edit_message_posts_json_payload() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":{\"message_id\":123}}".to_string()
        )]),
    );

    poller
        .edit_message(42, 123, "updated")
        .expect("edit message must succeed");

    let commands = &poller.executor.commands;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, "curl");
    assert_eq!(commands[0].1[2], "-X");
    assert_eq!(commands[0].1[3], "POST");
    assert!(commands[0].1[7].contains("\"chat_id\":42"));
    assert!(commands[0].1[7].contains("\"message_id\":123"));
    assert!(commands[0].1[7].contains("\"text\":\"updated\""));
    assert!(commands[0].1[8].contains("/bottoken_abc/editMessageText"));
}

#[test]
fn send_chat_action_posts_json_payload() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok("{\"ok\":true,\"result\":true}".to_string())]),
    );

    poller
        .send_chat_action(42, "typing")
        .expect("send chat action must succeed");

    let commands = &poller.executor.commands;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, "curl");
    assert_eq!(commands[0].1[2], "-X");
    assert_eq!(commands[0].1[3], "POST");
    assert_eq!(commands[0].1[4], "-H");
    assert_eq!(commands[0].1[5], "Content-Type: application/json");
    assert_eq!(commands[0].1[6], "-d");
    assert!(commands[0].1[7].contains("\"chat_id\":42"));
    assert!(commands[0].1[7].contains("\"action\":\"typing\""));
    assert!(commands[0].1[8].contains("/bottoken_abc/sendChatAction"));
}

#[test]
fn maps_telegram_api_error() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":false,\"description\":\"Forbidden\"}".to_string()
        )]),
    );

    let err = poller.get_updates(None, 5).expect_err("must fail");
    assert!(err.contains("Forbidden"));
}

#[test]
fn maps_message_reaction_updates() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":[{\"update_id\":6,\"message_reaction\":{\"chat\":{\"id\":42},\"message_id\":991,\"user\":{\"id\":1002},\"new_reaction\":[{\"type\":\"emoji\",\"emoji\":\"👍\"}]}}]}"
                .to_string(),
        )]),
    );

    let updates = poller
        .get_updates(None, 30)
        .expect("get updates must succeed");
    assert_eq!(updates.len(), 1);
    assert!(updates[0].message.is_none());
    assert_eq!(
        updates[0]
            .message_reaction
            .as_ref()
            .expect("reaction")
            .sender_user_id,
        Some(1002)
    );
    assert_eq!(
        updates[0]
            .message_reaction
            .as_ref()
            .expect("reaction")
            .emoji,
        vec!["👍".to_string()]
    );
}

#[test]
fn maps_voice_message_to_internal_prompt_marker() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":[{\"update_id\":10,\"message\":{\"message_id\":99,\"chat\":{\"id\":42},\"from\":{\"id\":1002},\"voice\":{\"file_id\":\"voice-file-1\"}}}]}"
                .to_string(),
        )]),
    );

    let updates = poller
        .get_updates(None, 30)
        .expect("get updates must succeed");
    assert_eq!(updates.len(), 1);
    let message = updates[0].message.as_ref().expect("voice marker message");
    assert_eq!(
        message.text,
        format!("{TELEGRAM_VOICE_PROMPT_PREFIX}voice-file-1")
    );
}

#[test]
fn maps_photo_message_to_internal_prompt_marker() {
    let mut poller = TelegramBotApiLongPoll::with_executor(
        "token_abc",
        "https://example.test",
        TestExecutor::new(vec![Ok(
            "{\"ok\":true,\"result\":[{\"update_id\":11,\"message\":{\"message_id\":100,\"chat\":{\"id\":42},\"from\":{\"id\":1002},\"photo\":[{\"file_id\":\"small\"},{\"file_id\":\"large\"}]}}]}"
                .to_string(),
        )]),
    );

    let updates = poller
        .get_updates(None, 30)
        .expect("get updates must succeed");
    assert_eq!(updates.len(), 1);
    let message = updates[0].message.as_ref().expect("photo marker message");
    assert_eq!(message.text, format!("{TELEGRAM_IMAGE_PROMPT_PREFIX}large"));
}
