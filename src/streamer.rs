use crate::resource::{RetryableError, RetryableResult};
use crate::Api;
use std::path::Path;
use tokio::process::Command;

/// Uses ffmpeg to stream a given m3u8 video file.
/// If there are multiple streams, ffmpeg automatically chooses the one with highest quality,
/// which is what we want.
pub async fn stream_video(
    api: &Api,
    stream_url_path: &str,
    temp_destination: &Path,
) -> RetryableResult<()> {
    stream_impl(
        api,
        move |cmd| cmd.arg("-i").arg(stream_url_path),
        temp_destination,
    )
    .await
}

#[derive(Debug, Clone)]
pub struct StreamSpec {
    pub stream_url_path: String,
    pub offset_seconds: f64,
}

/// Uses ffmpeg to stream multiple m3u8 video files and mux them together.
/// If there are multiple streams, ffmpeg automatically chooses the one with highest quality,
/// which is what we want.
pub async fn stream_and_mux_videos(
    api: &Api,
    streams: &[StreamSpec],
    temp_destination: &Path,
) -> RetryableResult<()> {
    if streams.len() == 1 {
        // if there's only one video, we should ignore the offset
        stream_video(api, &streams[0].stream_url_path, temp_destination).await
    } else {
        stream_impl(
            api,
            move |cmd| {
                let cmd = streams.iter().fold(cmd, move |cmd, s| {
                    cmd.arg("-itsoffset")
                        .arg(s.offset_seconds.to_string())
                        .arg("-i")
                        .arg(&s.stream_url_path)
                });
                streams
                    .iter()
                    .enumerate()
                    .fold(cmd, move |cmd, (i, _)| cmd.arg("-map").arg(i.to_string()))
            },
            temp_destination,
        )
        .await
    }
}

async fn stream_impl(
    api: &Api,
    input_args_appender: impl FnOnce(&mut Command) -> &mut Command,
    temp_destination: &Path,
) -> RetryableResult<()> {
    let success = input_args_appender(
        Command::new(&api.ffmpeg_path).arg("-y"), // flag to overwrite output file without prompting
    )
    .arg("-c")
    .arg("copy")
    .arg(temp_destination.as_os_str())
    .output()
    .await
    .map_err(|_| RetryableError::Fail("Failed to start ffmpeg"))?
    .status
    .success();
    if success {
        Ok(())
    } else {
        Err(RetryableError::Retry("ffmpeg returned nonzero exit code"))
    }
}
