use crate::{TelegramLongPoll, TelegramMessage, TelegramMessageReaction, TelegramUpdate};
use serde::{Deserialize, Serialize};
use std::process::Command;

pub struct TelegramBotApiLongPoll<E = SystemCommandExecutor>
where
    E: CommandExecutor,
{
    token: String,
    base_url: String,
    executor: E,
}

impl TelegramBotApiLongPoll<SystemCommandExecutor> {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            base_url: "https://api.telegram.org".to_string(),
            executor: SystemCommandExecutor,
        }
    }

    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            executor: SystemCommandExecutor,
        }
    }
}

impl<E> TelegramBotApiLongPoll<E>
where
    E: CommandExecutor,
{
    const ALLOWED_UPDATES_QUERY: &'static str =
        "allowed_updates=%5B%22message%22%2C%22message_reaction%22%5D";

    #[cfg(test)]
    fn with_executor(token: impl Into<String>, base_url: impl Into<String>, executor: E) -> Self {
        Self {
            token: token.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            executor,
        }
    }

    fn get_updates_url(&self, offset: Option<i64>, timeout_secs: u16) -> String {
        let mut url = format!(
            "{}/bot{}/getUpdates?timeout={}&{}",
            self.base_url,
            self.token,
            timeout_secs,
            Self::ALLOWED_UPDATES_QUERY
        );
        if let Some(offset) = offset {
            url.push_str(&format!("&offset={offset}"));
        }
        url
    }

    fn send_message_url(&self) -> String {
        format!("{}/bot{}/sendMessage", self.base_url, self.token)
    }

    fn send_chat_action_url(&self) -> String {
        format!("{}/bot{}/sendChatAction", self.base_url, self.token)
    }
}

impl<E> TelegramLongPoll for TelegramBotApiLongPoll<E>
where
    E: CommandExecutor,
{
    fn get_updates(
        &mut self,
        offset: Option<i64>,
        timeout_secs: u16,
    ) -> Result<Vec<TelegramUpdate>, String> {
        let raw = self.executor.run(
            "curl",
            &["-sS", "--fail", &self.get_updates_url(offset, timeout_secs)],
        )?;

        let response: TelegramApiResponse<Vec<TelegramGetUpdatesItem>> = serde_json::from_str(&raw)
            .map_err(|err| format!("telegram getUpdates decode failed: {err}"))?;

        response
            .into_result("getUpdates")?
            .into_iter()
            .map(map_update)
            .collect()
    }

    fn send_message(&mut self, chat_id: i64, text: &str) -> Result<Option<i64>, String> {
        let payload = serde_json::to_string(&TelegramSendMessageRequest { chat_id, text })
            .map_err(|err| format!("telegram sendMessage encode failed: {err}"))?;

        let raw = self.executor.run(
            "curl",
            &[
                "-sS",
                "--fail",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "-d",
                &payload,
                &self.send_message_url(),
            ],
        )?;

        let response: TelegramApiResponse<serde_json::Value> = serde_json::from_str(&raw)
            .map_err(|err| format!("telegram sendMessage decode failed: {err}"))?;

        let value = response.into_result("sendMessage")?;
        Ok(value.get("message_id").and_then(serde_json::Value::as_i64))
    }

    fn send_chat_action(&mut self, chat_id: i64, action: &str) -> Result<(), String> {
        let payload = serde_json::to_string(&TelegramSendChatActionRequest { chat_id, action })
            .map_err(|err| format!("telegram sendChatAction encode failed: {err}"))?;

        let raw = self.executor.run(
            "curl",
            &[
                "-sS",
                "--fail",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "-d",
                &payload,
                &self.send_chat_action_url(),
            ],
        )?;

        let response: TelegramApiResponse<serde_json::Value> = serde_json::from_str(&raw)
            .map_err(|err| format!("telegram sendChatAction decode failed: {err}"))?;
        let _ = response.into_result("sendChatAction")?;
        Ok(())
    }
}

pub trait CommandExecutor: Send {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String>;
}

pub struct SystemCommandExecutor;

impl CommandExecutor for SystemCommandExecutor {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|err| format!("failed to spawn {program}: {err}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!(
                "{program} failed: {}",
                if stderr.is_empty() {
                    "unknown error"
                } else {
                    &stderr
                }
            ));
        }

        String::from_utf8(output.stdout)
            .map_err(|err| format!("invalid utf-8 from {program}: {err}"))
    }
}

fn map_update(item: TelegramGetUpdatesItem) -> Result<TelegramUpdate, String> {
    let message = item.message.and_then(|message| {
        message.text.map(|text| TelegramMessage {
            chat_id: message.chat.id,
            sender_user_id: message.from.map(|user| user.id),
            message_id: message.message_id,
            reply_to_message_id: message.reply_to_message.map(|reply| reply.message_id),
            text,
        })
    });
    let message_reaction = item
        .message_reaction
        .map(|reaction| TelegramMessageReaction {
            chat_id: reaction.chat.id,
            sender_user_id: reaction.user.map(|user| user.id),
            message_id: reaction.message_id,
            emoji: reaction
                .new_reaction
                .into_iter()
                .filter_map(|reaction| match reaction.kind.as_str() {
                    "emoji" => reaction.emoji,
                    _ => None,
                })
                .collect(),
        });

    Ok(TelegramUpdate {
        update_id: item.update_id,
        message,
        message_reaction,
    })
}

#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

impl<T> TelegramApiResponse<T> {
    fn into_result(self, method: &str) -> Result<T, String> {
        if self.ok {
            return self
                .result
                .ok_or_else(|| format!("telegram {method} response missing result"));
        }

        Err(format!(
            "telegram {method} failed: {}",
            self.description
                .unwrap_or_else(|| "unknown telegram error".to_string())
        ))
    }
}

#[derive(Debug, Deserialize)]
struct TelegramGetUpdatesItem {
    update_id: i64,
    message: Option<TelegramIncomingMessage>,
    message_reaction: Option<TelegramIncomingMessageReaction>,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingMessage {
    message_id: i64,
    chat: TelegramIncomingChat,
    from: Option<TelegramIncomingUser>,
    text: Option<String>,
    reply_to_message: Option<TelegramIncomingMessageReply>,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingChat {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingMessageReply {
    message_id: i64,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingMessageReaction {
    chat: TelegramIncomingChat,
    message_id: i64,
    user: Option<TelegramIncomingUser>,
    #[serde(default)]
    new_reaction: Vec<TelegramIncomingReactionType>,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingUser {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingReactionType {
    #[serde(rename = "type")]
    kind: String,
    emoji: Option<String>,
}

#[derive(Debug, Serialize)]
struct TelegramSendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
}

#[derive(Debug, Serialize)]
struct TelegramSendChatActionRequest<'a> {
    chat_id: i64,
    action: &'a str,
}

#[cfg(test)]
mod tests {
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
            .send_message(42, "hi")
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
}
