use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use convert::{Format, Job, Report, convert, relevant_targets};
use gpui::Context;

use crate::app::{ConvertDemo, Item};

pub fn unique_dest(source: &Path, target: Format, folder: Option<&Path>) -> PathBuf {
    let parent = folder.unwrap_or_else(|| source.parent().unwrap_or_else(|| Path::new(".")));
    let dest = parent.join(dest_leaf(source, target));
    if !dest.exists() {
        return dest;
    }
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    let ext = target.ext();
    for n in 2..1000 {
        let candidate = parent.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dest
}

fn dest_leaf(source: &Path, target: Format) -> String {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    let ext = target.ext();
    let clashes = relevant_targets(target)
        .iter()
        .filter(|format| format.ext() == ext)
        .count()
        > 1;
    if clashes {
        format!(
            "{stem}.converted.{}.{ext}",
            target.label().to_ascii_lowercase()
        )
    } else {
        format!("{stem}.converted.{ext}")
    }
}

pub fn start(this: &mut ConvertDemo, cx: &mut Context<ConvertDemo>, only: Option<usize>) {
    if this.is_busy || this.items.is_empty() {
        return;
    }
    let target = this.target;
    let intent = this.intent;
    let keep = this.should_keep_metadata;
    let folder = this.output_dir.clone();
    let mut jobs = Vec::new();
    let mut skipped_kind = 0;
    for (index, item) in this.items.iter_mut().enumerate() {
        if only.is_some_and(|only| only != index) {
            continue;
        }
        let Some(probe) = item.probe.as_ref() else {
            continue;
        };
        if probe.kind != target.kind() {
            if only.is_some() {
                item.error = Some(format!(
                    "This is {}",
                    match probe.kind {
                        convert::Kind::Image => "a photo",
                        convert::Kind::Audio => "audio",
                        convert::Kind::Font => "a font",
                    }
                ));
            } else {
                skipped_kind += 1;
            }
            continue;
        }
        if only.is_none() && item.report.is_some() {
            continue;
        }
        item.is_busy = true;
        item.error = None;
        item.report = None;
        let dest = unique_dest(&item.path, target, folder.as_deref());
        item.dest = Some(dest.clone());
        jobs.push((
            index,
            Job {
                source: item.path.clone(),
                dest,
                target,
                intent,
                max_edge: intent.default_max_edge(),
                keep_metadata: keep,
            },
        ));
    }
    this.notice = if skipped_kind > 0 {
        Some(format!(
            "{skipped_kind} {} left out of this batch",
            if skipped_kind == 1 {
                "file was"
            } else {
                "files were"
            }
        ))
    } else {
        None
    };
    this.summary = None;
    if jobs.is_empty() {
        return;
    }
    this.is_busy = true;
    this.done = 0;
    this.total = jobs.len();
    this.cancel.store(false, Ordering::Relaxed);
    let cancel = this.cancel.clone();
    cx.spawn(async move |this, cx| {
        for (index, job) in jobs {
            if cancel.load(Ordering::Relaxed) {
                this.update(cx, |this, cx| {
                    for item in &mut this.items {
                        item.is_busy = false;
                    }
                    this.is_busy = false;
                    this.notice = Some("Stopped.".into());
                    this.finish_summary();
                    cx.notify();
                })
                .ok();
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move { convert(&job) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(item) = this.items.get_mut(index) {
                    item.is_busy = false;
                    match result {
                        Ok(report) => apply_report(item, report),
                        Err(err) => item.error = Some(plain_error(&err.to_string())),
                    }
                }
                this.done += 1;
                if this.done >= this.total {
                    this.is_busy = false;
                    this.finish_summary();
                }
                cx.notify();
            })
            .ok();
        }
    })
    .detach();
}

fn apply_report(item: &mut Item, report: Report) {
    item.report = Some(report);
}

fn plain_error(err: &str) -> String {
    if err.contains("avifenc") {
        "Can't write AVIF on this Mac".into()
    } else if err.contains("ffmpeg") {
        "Can't convert audio — ffmpeg isn't installed".into()
    } else {
        err.to_string()
    }
}
