use super::*;
use crate::media as app_media;
#[test]
fn st_audio_stt_args_include_input_path() {
    let args = app_media::st_audio_stt_args(Path::new("/tmp/input.ogg"));
    let rendered = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<String>>();
    assert_eq!(
        rendered,
        vec!["stt".to_string(), "/tmp/input.ogg".to_string()]
    );
}

#[test]
fn st_audio_tts_args_force_opus_format() {
    let args =
        app_media::st_audio_tts_args(Path::new("/tmp/tts-input.txt"), Path::new("/tmp/out.ogg"));
    let rendered = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<String>>();
    assert_eq!(
        rendered,
        vec![
            "tts".to_string(),
            "/tmp/tts-input.txt".to_string(),
            "--format".to_string(),
            "opus".to_string(),
            "--output".to_string(),
            "/tmp/out.ogg".to_string(),
        ]
    );
}

#[test]
fn codex_image_ocr_args_include_read_only_ephemeral_image_prompt() {
    let args = app_media::codex_image_ocr_args(Path::new("/tmp/photo.png"));
    let rendered = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<String>>();
    assert_eq!(
        rendered,
        vec![
            "exec".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            "--ephemeral".to_string(),
            "--image".to_string(),
            "/tmp/photo.png".to_string(),
            "--".to_string(),
            app_media::CODEX_IMAGE_OCR_PROMPT.to_string(),
        ]
    );
}
