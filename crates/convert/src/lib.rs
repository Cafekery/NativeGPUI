//! Probe a file, pick copy / remux / encode, then write a report. Nothing here knows GPUI.

mod audio;
mod font;
mod format;
mod image;
mod plan;
mod probe;
mod report;

pub use format::{Format, Kind, Outline};
pub use plan::{Intent, PathTaken, Planned};
pub use probe::Probe;
pub use report::Report;

use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

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

pub fn probe(path: &Path) -> Result<Probe> {
    probe::probe(path)
}

pub fn plan(probe: &Probe, target: Format, intent: Intent) -> Result<Planned> {
    plan::plan(probe, target, intent)
}

pub fn relevant_targets(format: Format) -> &'static [Format] {
    format::relevant_targets(format)
}

pub fn ffmpeg_available() -> bool {
    audio::ffmpeg_available()
}

pub fn convert(job: &Job) -> Result<Report> {
    let probed = probe::probe(&job.source)?;
    let planned = plan::plan(&probed, job.target, job.intent)?;
    if probed.kind != job.target.kind() {
        return Err(Error::new("source and target are different kinds of media"));
    }
    let dest_parent = job
        .dest
        .parent()
        .ok_or_else(|| Error::new("destination has no parent"))?;
    std::fs::create_dir_all(dest_parent).map_err(|e| Error::new(e.to_string()))?;

    match probed.kind {
        Kind::Image => image::run(job, &probed, &planned),
        Kind::Audio => audio::run(job, &probed, &planned),
        Kind::Font => font::run(job, &probed, &planned),
    }
}

pub fn dest_name(source: &Path, target: Format) -> PathBuf {
    let stem = source.file_stem().unwrap_or_default();
    source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            "{}.converted.{}",
            stem.to_string_lossy(),
            target.ext()
        ))
}

#[cfg(test)]
mod tests;
