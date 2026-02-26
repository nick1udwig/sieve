use crate::{TelegramLongPoll, TelegramMessage, TelegramUpdate};
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
            "{}/bot{}/getUpdates?timeout={}",
            self.base_url, self.token, timeout_secs
        );
        if let Some(offset) = offset {
            url.push_str(&format!("&offset={offset}"));
        }
        url
    }

    fn send_message_url(&self) -> String {
        format!("{}/bot{}/sendMessage", self.base_url, self.token)
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

    fn send_message(&mut self, chat_id: i64, text: &str) -> Result<(), String> {
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

        response.into_result("sendMessage")?;
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
            text,
        })
    });

    Ok(TelegramUpdate {
        update_id: item.update_id,
        message,
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
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingMessage {
    chat: TelegramIncomingChat,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingChat {
    id: i64,
}

#[derive(Debug, Serialize)]
struct TelegramSendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
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
                "{\"ok\":true,\"result\":[{\"update_id\":5,\"message\":{\"chat\":{\"id\":42},\"text\":\"/approve apr_1\"}}]}"
                    .to_string(),
            )]),
        );

        let updates = poller
            .get_updates(Some(9), 30)
            .expect("get updates must succeed");

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_id, 5);
        assert_eq!(updates[0].message.as_ref().expect("msg").chat_id, 42);

        let commands = &poller.executor.commands;
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "curl");
        assert_eq!(commands[0].1[0], "-sS");
        assert_eq!(commands[0].1[1], "--fail");
        assert!(commands[0].1[2].contains("/bottoken_abc/getUpdates?timeout=30&offset=9"));
    }

    #[test]
    fn send_message_posts_json_payload() {
        let mut poller = TelegramBotApiLongPoll::with_executor(
            "token_abc",
            "https://example.test",
            TestExecutor::new(vec![Ok("{\"ok\":true,\"result\":{}}".to_string())]),
        );

        poller
            .send_message(42, "hi")
            .expect("send message must succeed");

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
}
