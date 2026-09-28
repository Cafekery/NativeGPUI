use std::path::Path;
use std::process::Command;

use crate::{Error, Format, Job, PathTaken, Planned, Probe, Report, Result};

pub fn ffmpeg_available() -> bool {
    runs("ffmpeg") && runs("ffprobe")
}

fn runs(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

pub fn ffprobe(path: &Path) -> Option<(Option<f64>, Option<String>)> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,duration",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut duration = None;
    let mut codec = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(name) = line.strip_prefix("codec_name=") {
            if name != "N/A" && !name.is_empty() {
                codec = Some(name.to_string());
            }
        } else if let Some(Ok(seconds)) = line.strip_prefix("duration=").map(str::parse) {
            duration = Some(seconds);
        }
    }
    Some((duration, codec))
}

pub fn run(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    if !ffmpeg_available() {
        return Err(Error::new("ffmpeg and ffprobe are not on PATH"));
    }
    if planned.path == PathTaken::Copy {
        return crate::copy(job, probe, planned);
    }

    let dest = job.dest.with_extension(job.target.ext());
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(&job.source);
    if planned.path == PathTaken::Remux {
        cmd.args(["-c", "copy"]);
    } else {
        let (codec, lossy) = match job.target {
            Format::Wav => ("pcm_s16le", false),
            Format::Flac => ("flac", false),
            Format::Alac => ("alac", false),
            Format::Mp3 => ("libmp3lame", true),
            Format::Aac | Format::M4a | Format::Mp4 => ("aac", true),
            Format::Opus => ("libopus", true),
            _ => return Err(Error::new("internal: wrong encoder for target")),
        };
        cmd.args(["-c:a", codec]);
        if lossy {
            let bitrate = format!("{}k", job.intent.audio_bitrate_k(job.target));
            cmd.args(["-b:a", &bitrate]);
        }
    }
    if !job.keep_metadata {
        cmd.args(["-map_metadata", "-1"]);
    }
    let out = cmd.arg(&dest).output()?;
    if !out.status.success() {
        return Err(Error::new(format!(
            "ffmpeg: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let mut report = Report::new(
        probe,
        planned.path,
        std::fs::metadata(&dest)?.len(),
        planned.notes.clone(),
    );
    if let Some((duration, _)) = ffprobe(&dest) {
        report.duration_out = duration;
    }
    Ok(report)
}
