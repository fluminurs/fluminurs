use crate::resource::{RetryableError, RetryableResult};
use crate::Api;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
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
    assert!(!streams.is_empty());
    if streams.len() == 1 {
        // if there's only one video, we should ignore the offset
        stream_video(api, &streams[0].stream_url_path, temp_destination).await
    } else {
        // we have multiple videos, we have to stream each of them to separate temporary files, then mux them together
        // the reason why we need temp files is here:
        // https://stackoverflow.com/questions/68890149/download-multiple-files-with-ffmpeg-keep-one-stream-from-each-according-to-def

        // generate the temp file names
        let temp_stream_dests: Vec<PathBuf> = (0..streams.len())
            .map(|i| make_temp_stream_file_name(temp_destination, i))
            .collect();

        // stream the streams to the temp files
        let stream_results: Vec<RetryableResult<()>> = futures_util::future::join_all(
            streams
                .iter()
                .zip(temp_stream_dests.iter())
                .map(|(s, dest)| stream_video(api, &s.stream_url_path, dest)),
        )
        .await;
        // throw RetryableError::Fail if any
        stream_results
            .iter()
            .copied()
            .find(|sr| matches!(sr, Err(RetryableError::Fail(_))))
            .transpose()?;
        // throw RetryableError if any
        stream_results
            .iter()
            .copied()
            .find(|sr| matches!(sr, Err(_)))
            .transpose()?;

        // now we know that all downloads succeeded

        // mux the temp files
        let temp_stream_dests_ref = temp_stream_dests.as_slice();
        stream_impl(
            api,
            move |cmd| {
                let cmd = streams.iter().zip(temp_stream_dests_ref.iter()).fold(
                    cmd,
                    move |cmd, (s, tsd)| {
                        cmd.arg("-itsoffset")
                            .arg(s.offset_seconds.to_string())
                            .arg("-i")
                            .arg(tsd.as_path())
                    },
                );
                (0..streams.len()).fold(cmd, move |cmd, i| cmd.arg("-map").arg(i.to_string()))
            },
            temp_destination,
        )
        .await?;

        // delete the temp files (but silence the error if not possible)
        futures_util::future::join_all(temp_stream_dests.into_iter().map(tokio::fs::remove_file))
            .await;

        Ok(())
    }
}

fn make_temp_stream_file_name(name: &Path, index: usize) -> PathBuf {
    let old_filename = name.file_name().expect("Path needs file name");
    let prepend = OsStr::new("~!");
    let index_string = index.to_string();
    let after_prepend = prepend;
    let index_osstr = OsStr::new(&index_string);
    let mut new_filename = OsString::with_capacity(
        prepend.len() + index_osstr.len() + after_prepend.len() + old_filename.len(),
    );
    new_filename.push(prepend);
    new_filename.push(index_osstr);
    new_filename.push(after_prepend);
    new_filename.push(old_filename);
    let mut res = name.to_path_buf();
    res.set_file_name(new_filename);
    res
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
