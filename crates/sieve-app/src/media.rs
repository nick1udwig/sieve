use crate::config::AppConfig;
use sieve_runtime::CodexTool;
use sieve_types::{CodexSandboxMode, CodexSessionRequest, RunId};
use std::path::{Path, PathBuf};
use tokio::process::Command as TokioCommand;

pub(crate) const CODEX_IMAGE_OCR_PROMPT: &str = include_str!("prompts/codex_image_ocr.md");
const ST_TTS_OUTPUT_FORMAT: &str = "opus";

fn command_error_from_output(context: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{context} failed")
    } else {
        format!("{context} failed: {stderr}")
    }
}

pub(crate) fn codex_image_ocr_task_request(input_path: &Path) -> CodexSessionRequest {
    CodexSessionRequest {
        session_id: None,
        instruction: CODEX_IMAGE_OCR_PROMPT.to_string(),
        sandbox: CodexSandboxMode::ReadOnly,
        cwd: None,
        writable_roots: Vec::new(),
        local_images: vec![input_path.to_string_lossy().to_string()],
    }
}

pub(crate) fn st_audio_stt_args(input_path: &Path) -> Vec<std::ffi::OsString> {
    vec!["stt".into(), input_path.as_os_str().to_owned()]
}

pub(crate) fn st_audio_tts_args(text_path: &Path, output_path: &Path) -> Vec<std::ffi::OsString> {
    vec![
        "tts".into(),
        text_path.as_os_str().to_owned(),
        "--format".into(),
        ST_TTS_OUTPUT_FORMAT.into(),
        "--output".into(),
        output_path.as_os_str().to_owned(),
    ]
}

async fn fetch_telegram_file_path(bot_token: &str, file_id: &str) -> Result<String, String> {
    let url = format!("https://api.telegram.org/bot{bot_token}/getFile");
    let output = TokioCommand::new("curl")
        .arg("-sS")
        .arg("--fail")
        .arg("--get")
        .arg("--data-urlencode")
        .arg(format!("file_id={file_id}"))
        .arg(url)
        .output()
        .await
        .map_err(|err| format!("failed to fetch telegram file metadata: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("telegram getFile", &output));
    }
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("invalid telegram getFile response: {err}"))?;
    payload
        .pointer("/result/file_path")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| "telegram getFile response missing result.file_path".to_string())
}

async fn download_telegram_file(
    bot_token: &str,
    file_path: &str,
    destination: &Path,
) -> Result<(), String> {
    let url = format!("https://api.telegram.org/file/bot{bot_token}/{file_path}");
    let output = TokioCommand::new("curl")
        .arg("-sS")
        .arg("--fail")
        .arg("-o")
        .arg(destination)
        .arg(url)
        .output()
        .await
        .map_err(|err| format!("failed to download telegram file: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("telegram file download", &output));
    }
    Ok(())
}

pub(crate) async fn transcribe_audio_prompt(
    cfg: &AppConfig,
    run_id: &RunId,
    file_id: &str,
) -> Result<String, String> {
    let file_path = fetch_telegram_file_path(&cfg.telegram_bot_token, file_id).await?;
    let media_dir = cfg.sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let ext = Path::new(&file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .unwrap_or("ogg");
    let input_path = media_dir.join(format!("voice-input.{ext}"));
    download_telegram_file(&cfg.telegram_bot_token, &file_path, &input_path).await?;

    let mut command = TokioCommand::new("st");
    for arg in st_audio_stt_args(&input_path) {
        command.arg(arg);
    }
    let output = command
        .output()
        .await
        .map_err(|err| format!("audio STT command spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("audio STT command", &output));
    }
    let transcript = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if transcript.is_empty() {
        return Err("audio STT command produced empty transcript".to_string());
    }
    Ok(transcript)
}

pub(crate) async fn extract_image_prompt(
    codex: &dyn CodexTool,
    bot_token: &str,
    sieve_home: &Path,
    run_id: &RunId,
    file_id: &str,
) -> Result<String, String> {
    let file_path = fetch_telegram_file_path(bot_token, file_id).await?;
    let media_dir = sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let ext = Path::new(&file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .unwrap_or("jpg");
    let input_path = media_dir.join(format!("image-input.{ext}"));
    download_telegram_file(bot_token, &file_path, &input_path).await?;

    let result = codex
        .run_task(codex_image_ocr_task_request(&input_path))
        .await?;
    let extracted = result
        .result
        .user_visible
        .clone()
        .unwrap_or(result.result.summary)
        .trim()
        .to_string();
    if extracted.is_empty() {
        return Err("image OCR command produced empty output".to_string());
    }
    Ok(extracted)
}

pub(crate) async fn synthesize_audio_reply(
    cfg: &AppConfig,
    run_id: &RunId,
    assistant_message: &str,
) -> Result<PathBuf, String> {
    let media_dir = cfg.sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let text_path = media_dir.join("tts-input.txt");
    let output_path = media_dir.join("tts-output.ogg");
    tokio::fs::write(&text_path, assistant_message)
        .await
        .map_err(|err| format!("failed to write TTS input text: {err}"))?;

    let mut command = TokioCommand::new("st");
    for arg in st_audio_tts_args(&text_path, &output_path) {
        command.arg(arg);
    }
    let output = command
        .output()
        .await
        .map_err(|err| format!("audio TTS command spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("audio TTS command", &output));
    }

    let metadata = tokio::fs::metadata(&output_path)
        .await
        .map_err(|err| format!("audio TTS output missing: {err}"))?;
    if metadata.len() == 0 {
        return Err("audio TTS output file is empty".to_string());
    }
    Ok(output_path)
}

pub(crate) async fn send_telegram_voice(
    bot_token: &str,
    chat_id: i64,
    audio_path: &Path,
) -> Result<(), String> {
    let endpoint = format!("https://api.telegram.org/bot{bot_token}/sendVoice");
    let voice_arg = format!("voice=@{}", audio_path.to_string_lossy());
    let output = TokioCommand::new("curl")
        .arg("-sS")
        .arg("--fail")
        .arg("-X")
        .arg("POST")
        .arg("-F")
        .arg(format!("chat_id={chat_id}"))
        .arg("-F")
        .arg(voice_arg)
        .arg(endpoint)
        .output()
        .await
        .map_err(|err| format!("failed to send telegram voice message: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("telegram sendVoice", &output));
    }
    Ok(())
}
