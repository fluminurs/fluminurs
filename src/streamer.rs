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
    let success = Command::new(&api.ffmpeg_path)
        .arg("-y") // flag to overwrite output file without prompting
        .arg("-i")
        .arg(stream_url_path)
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
