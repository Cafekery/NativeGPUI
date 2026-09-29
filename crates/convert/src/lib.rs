mod audio;
mod font;
mod format;
mod image;
mod plan;
mod probe;
mod report;

pub use audio::ffmpeg_available;
pub use format::{Format, Kind, Outline, relevant_targets};
pub use plan::{Intent, PathTaken, Planned, plan};
pub use probe::{Probe, probe};
pub use report::Report;

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub(crate) fn new(message: impl fmt::Display) -> Self {
        Self(message.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(e)
    }
}

impl From<::image::ImageError> for Error {
    fn from(e: ::image::ImageError) -> Self {
        Self::new(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct Job {
    pub source: PathBuf,
    pub dest: PathBuf,
    pub target: Format,
    pub intent: Intent,
    pub max_edge: Option<u32>,
    pub keep_metadata: bool,
}

pub fn convert(job: &Job) -> Result<Report> {
    let probed = probe(&job.source)?;
    let planned = plan(&probed, job.target, job.intent)?;
    let dest_parent = job
        .dest
        .parent()
        .ok_or_else(|| Error::new("destination has no parent"))?;
    std::fs::create_dir_all(dest_parent)?;
    match probed.kind {
        Kind::Image => image::run(job, &probed, &planned),
        Kind::Audio => audio::run(job, &probed, &planned),
        Kind::Font => font::run(job, &probed, &planned),
    }
}

fn copy(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    let bytes_out = std::fs::copy(&job.source, &job.dest)?;
    Ok(Report::new(
        probe,
        PathTaken::Copy,
        bytes_out,
        planned.notes.clone(),
    ))
}

#[cfg(test)]
mod tests;
