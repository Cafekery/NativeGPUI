use std::fs;
use std::path::Path;
use std::process::Command;

use crate::plan::{PathTaken, Planned};
use crate::report::Report;
use crate::{Error, Format, Job, Probe, Result};

pub fn ffmpeg_available() -> bool {
    which("ffmpeg") && which("ffprobe")
}

fn which(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
    let text = String::from_utf8_lossy(&out.stdout);
    let mut codec = None;
    let mut duration = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("codec_name=") {
            if rest != "N/A" && !rest.is_empty() {
                codec = Some(rest.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("duration=") {
            if rest != "N/A" {
                if let Ok(v) = rest.parse::<f64>() {
                    duration = Some(v);
                }
            }
        }
    }
    Some((duration, codec))
}

pub fn run(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    if !ffmpeg_available() {
        return Err(Error::new("ffmpeg and ffprobe are not on PATH"));
    }
    match planned.path {
        PathTaken::Copy => copy_file(job, probe, planned),
        PathTaken::Remux => ffmpeg(job, probe, planned, true),
        PathTaken::Encode => ffmpeg(job, probe, planned, false),
    }
}

fn copy_file(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    fs::copy(&job.source, &job.dest).map_err(|e| Error::new(e.to_string()))?;
    let bytes_out = fs::metadata(&job.dest)
        .map_err(|e| Error::new(e.to_string()))?
        .len();
    Ok(Report {
        bytes_in: probe.bytes,
        bytes_out,
        width_in: None,
        height_in: None,
        width_out: None,
        height_out: None,
        duration_in: probe.duration,
        duration_out: probe.duration,
        path: PathTaken::Copy,
        notes: planned.notes.clone(),
    })
}

fn ffmpeg(job: &Job, probe: &Probe, planned: &Planned, remux: bool) -> Result<Report> {
    let dest_ext = job.target.ext();
    let dest = if job.dest.extension().and_then(|e| e.to_str()) == Some(dest_ext) {
        job.dest.clone()
    } else {
        job.dest.with_extension(dest_ext)
    };

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]);
    cmd.arg(&job.source);
    if remux {
        cmd.args(["-c", "copy"]);
    } else {
        encode_args(&mut cmd, job.target, job.intent.audio_bitrate_k(job.target));
    }
    if !job.keep_metadata {
        cmd.args(["-map_metadata", "-1"]);
    }
    cmd.arg(&dest);
    let out = cmd.output().map_err(|e| Error::new(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::new(format!(
            "ffmpeg: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let bytes_out = fs::metadata(&dest)
        .map_err(|e| Error::new(e.to_string()))?
        .len();
    let (duration_out, _) = ffprobe(&dest).unwrap_or((probe.duration, None));
    Ok(Report {
        bytes_in: probe.bytes,
        bytes_out,
        width_in: None,
        height_in: None,
        width_out: None,
        height_out: None,
        duration_in: probe.duration,
        duration_out,
        path: if remux {
            PathTaken::Remux
        } else {
            PathTaken::Encode
        },
        notes: planned.notes.clone(),
    })
}

fn encode_args(cmd: &mut Command, target: Format, bitrate_k: u32) {
    let br = format!("{bitrate_k}k");
    match target {
        Format::Wav => {
            cmd.args(["-c:a", "pcm_s16le"]);
        }
        Format::Flac => {
            cmd.args(["-c:a", "flac"]);
        }
        Format::Alac => {
            cmd.args(["-c:a", "alac"]);
        }
        Format::Mp3 => {
            cmd.args(["-c:a", "libmp3lame", "-b:a", &br]);
        }
        Format::Aac | Format::M4a | Format::Mp4 => {
            cmd.args(["-c:a", "aac", "-b:a", &br]);
        }
        Format::Opus => {
            cmd.args(["-c:a", "libopus", "-b:a", &br]);
        }
        Format::Png
        | Format::Jpeg
        | Format::Webp
        | Format::Heic
        | Format::Avif
        | Format::Tiff
        | Format::Gif
        | Format::Ttf
        | Format::Otf
        | Format::Woff
        | Format::Woff2 => {}
    }
}

/// Raw little-endian s16 mono samples, for tests.
#[cfg(test)]
pub fn decode_pcm_s16(path: &Path) -> Result<Vec<u8>> {
    if !ffmpeg_available() {
        return Err(Error::new("ffmpeg is not on PATH"));
    }
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "1",
            "-ar",
            "44100",
            "-",
        ])
        .output()
        .map_err(|e| Error::new(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::new(format!(
            "ffmpeg decode: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}
