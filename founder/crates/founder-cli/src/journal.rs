use std::path::{Path, PathBuf};

use finder_fs::ops::transfer::{Kind, OnError, Request};
use finder_fs::ops::{Conflict, ImageFormat, ImageSize, Op, Quick, Restore};
use serde::{Deserialize, Serialize};

use crate::Stop;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Step {
    Move {
        items: Vec<PathBuf>,
        into: PathBuf,
    },
    Copy {
        items: Vec<PathBuf>,
        into: PathBuf,
    },
    CopyExactly {
        items: Vec<PathBuf>,
        into: PathBuf,
    },
    Trash {
        items: Vec<PathBuf>,
    },
    DeleteImmediately {
        items: Vec<PathBuf>,
    },
    Rename {
        path: PathBuf,
        name: String,
    },
    Duplicate {
        items: Vec<PathBuf>,
    },
    MakeAlias {
        items: Vec<PathBuf>,
    },
    Compress {
        items: Vec<PathBuf>,
    },
    NewFolder {
        into: PathBuf,
        name: String,
    },
    NewFile {
        into: PathBuf,
        name: String,
    },
    PutBack {
        items: Vec<Back>,
    },
    Retry {
        kind: Carry,
        pairs: Vec<Pair>,
        conflict: Policy,
        on_error: Errors,
    },
    Rotate {
        items: Vec<PathBuf>,
        left: bool,
    },
    CreatePdf {
        items: Vec<PathBuf>,
    },
    Convert {
        items: Vec<PathBuf>,
        format: Format,
        size: Size,
        metadata: bool,
    },
    RemoveBackground {
        items: Vec<PathBuf>,
    },
    Trim {
        item: PathBuf,
        start: f64,
        end: f64,
        into: PathBuf,
        tags: Vec<String>,
    },
    Batch {
        ops: Vec<Step>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Back {
    pub item: PathBuf,
    pub to: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pair {
    pub source: PathBuf,
    pub dest: PathBuf,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Carry {
    Copy,
    Move,
    CopyExactly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    #[default]
    KeepBoth,
    Replace,
    Merge,
    Skip,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Errors {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    Jpeg,
    Png,
    Heif,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Size {
    Actual,
    Large,
    Medium,
    Small,
}

impl Policy {
    pub fn parse(text: &str) -> Option<Policy> {
        Some(match text {
            "keep-both" => Policy::KeepBoth,
            "replace" => Policy::Replace,
            "merge" => Policy::Merge,
            "skip" => Policy::Skip,
            _ => return None,
        })
    }

    pub fn conflict(self) -> Conflict {
        match self {
            Policy::KeepBoth => Conflict::KeepBoth,
            Policy::Replace => Conflict::Replace,
            Policy::Merge => Conflict::Merge,
            Policy::Skip => Conflict::Skip,
        }
    }

    pub fn of(conflict: Conflict) -> Policy {
        match conflict {
            Conflict::KeepBoth => Policy::KeepBoth,
            Conflict::Replace => Policy::Replace,
            Conflict::Merge => Policy::Merge,
            Conflict::Skip => Policy::Skip,
        }
    }
}

impl Errors {
    pub fn on_error(self) -> OnError {
        match self {
            Errors::Continue => OnError::Continue,
            Errors::Stop => OnError::Stop,
        }
    }
}

impl Format {
    pub fn parse(text: &str) -> Option<Format> {
        Some(match text.to_ascii_lowercase().as_str() {
            "jpeg" | "jpg" => Format::Jpeg,
            "png" => Format::Png,
            "heif" | "heic" => Format::Heif,
            _ => return None,
        })
    }

    fn image(self) -> ImageFormat {
        match self {
            Format::Jpeg => ImageFormat::Jpeg,
            Format::Png => ImageFormat::Png,
            Format::Heif => ImageFormat::Heif,
        }
    }
}

impl Size {
    pub fn parse(text: &str) -> Option<Size> {
        Some(match text.to_ascii_lowercase().as_str() {
            "actual" => Size::Actual,
            "large" => Size::Large,
            "medium" => Size::Medium,
            "small" => Size::Small,
            _ => return None,
        })
    }

    fn image(self) -> ImageSize {
        match self {
            Size::Actual => ImageSize::Actual,
            Size::Large => ImageSize::Large,
            Size::Medium => ImageSize::Medium,
            Size::Small => ImageSize::Small,
        }
    }
}

impl From<&Op> for Step {
    fn from(op: &Op) -> Step {
        match op.clone() {
            Op::Move { items, into } => Step::Move { items, into },
            Op::Copy { items, into } => Step::Copy { items, into },
            Op::CopyExactly { items, into } => Step::CopyExactly { items, into },
            Op::Trash(items) => Step::Trash { items },
            Op::DeleteImmediately(items) => Step::DeleteImmediately { items },
            Op::Rename { path, name } => Step::Rename { path, name },
            Op::Duplicate(items) => Step::Duplicate { items },
            Op::MakeAlias(items) => Step::MakeAlias { items },
            Op::Compress(items) => Step::Compress { items },
            Op::NewFolder { into, name } => Step::NewFolder { into, name },
            Op::NewFile { into, name } => Step::NewFile { into, name },
            Op::PutBack(items) => Step::PutBack {
                items: (items.into_iter())
                    .map(|restore| Back {
                        item: restore.item,
                        to: restore.to,
                    })
                    .collect(),
            },
            Op::Retry(request) => Step::Retry {
                kind: match request.kind() {
                    Kind::Copy => Carry::Copy,
                    Kind::Move => Carry::Move,
                    Kind::CopyExactly => Carry::CopyExactly,
                },
                pairs: (request.items())
                    .map(|(source, dest)| Pair {
                        source: source.to_path_buf(),
                        dest: dest.to_path_buf(),
                    })
                    .collect(),
                conflict: Policy::of(request.conflict()),
                on_error: match request.on_error() {
                    OnError::Continue => Errors::Continue,
                    OnError::Stop => Errors::Stop,
                },
            },
            Op::Quick(quick) => match quick {
                Quick::Rotate { items, left } => Step::Rotate { items, left },
                Quick::CreatePdf(items) => Step::CreatePdf { items },
                Quick::Convert {
                    items,
                    format,
                    size,
                    metadata,
                } => Step::Convert {
                    items,
                    format: match format {
                        ImageFormat::Jpeg => Format::Jpeg,
                        ImageFormat::Png => Format::Png,
                        ImageFormat::Heif => Format::Heif,
                    },
                    size: match size {
                        ImageSize::Actual => Size::Actual,
                        ImageSize::Large => Size::Large,
                        ImageSize::Medium => Size::Medium,
                        ImageSize::Small => Size::Small,
                    },
                    metadata,
                },
                Quick::RemoveBackground(items) => Step::RemoveBackground { items },
                Quick::Trim {
                    item,
                    start,
                    end,
                    into,
                    tags,
                } => Step::Trim {
                    item,
                    start,
                    end,
                    into,
                    tags,
                },
            },
            Op::Batch(ops) => Step::Batch {
                ops: ops.iter().map(Step::from).collect(),
            },
        }
    }
}

impl From<Step> for Op {
    fn from(step: Step) -> Op {
        match step {
            Step::Move { items, into } => Op::Move { items, into },
            Step::Copy { items, into } => Op::Copy { items, into },
            Step::CopyExactly { items, into } => Op::CopyExactly { items, into },
            Step::Trash { items } => Op::Trash(items),
            Step::DeleteImmediately { items } => Op::DeleteImmediately(items),
            Step::Rename { path, name } => Op::Rename { path, name },
            Step::Duplicate { items } => Op::Duplicate(items),
            Step::MakeAlias { items } => Op::MakeAlias(items),
            Step::Compress { items } => Op::Compress(items),
            Step::NewFolder { into, name } => Op::NewFolder { into, name },
            Step::NewFile { into, name } => Op::NewFile { into, name },
            Step::PutBack { items } => Op::PutBack(
                items
                    .into_iter()
                    .map(|back| Restore {
                        item: back.item,
                        to: back.to,
                    })
                    .collect(),
            ),
            Step::Retry {
                kind,
                pairs,
                conflict,
                on_error,
            } => Op::Retry(Request::each(
                match kind {
                    Carry::Copy => Kind::Copy,
                    Carry::Move => Kind::Move,
                    Carry::CopyExactly => Kind::CopyExactly,
                },
                pairs.into_iter().map(|pair| (pair.source, pair.dest)),
                conflict.conflict(),
                on_error.on_error(),
            )),
            Step::Rotate { items, left } => Op::Quick(Quick::Rotate { items, left }),
            Step::CreatePdf { items } => Op::Quick(Quick::CreatePdf(items)),
            Step::Convert {
                items,
                format,
                size,
                metadata,
            } => Op::Quick(Quick::Convert {
                items,
                format: format.image(),
                size: size.image(),
                metadata,
            }),
            Step::RemoveBackground { items } => Op::Quick(Quick::RemoveBackground(items)),
            Step::Trim {
                item,
                start,
                end,
                into,
                tags,
            } => Op::Quick(Quick::Trim {
                item,
                start,
                end,
                into,
                tags,
            }),
            Step::Batch { ops } => Op::Batch(ops.into_iter().map(Op::from).collect()),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    recorded: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub label: String,
    pub undo: Step,
    pub redo: Step,
    pub serial: u64,
    /// What a taken name meant the first time; a redo means it again.
    #[serde(default)]
    pub conflict: Policy,
    /// What a failed or stopped run did not get to, for `founder retry`.
    #[serde(default)]
    pub left: Option<Step>,
}

impl Journal {
    pub fn load(path: &Path) -> Result<Journal, Stop> {
        match std::fs::read(path) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(Journal::default()),
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                Stop::Refused(format!("{} is not a journal: {error}", path.display()))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(error) => Err(Stop::Failed(format!("{}: {error}", path.display()))),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), Stop> {
        let text = serde_json::to_string_pretty(self).expect("a journal always serializes");
        let temp = path.with_extension("founder-journal-tmp");
        std::fs::write(&temp, text + "\n")
            .and_then(|()| std::fs::rename(&temp, path))
            .map_err(|error| Stop::Failed(format!("{}: {error}", path.display())))
    }

    pub fn record(
        &mut self,
        label: String,
        undo: Option<&Op>,
        original: &Op,
        conflict: Conflict,
        left: Option<&Op>,
    ) {
        let Some(undo) = undo else {
            return;
        };
        self.recorded += 1;
        self.undo.push(Entry {
            label,
            undo: Step::from(undo),
            redo: Step::from(original),
            serial: self.recorded,
            conflict: Policy::of(conflict),
            left: left.map(Step::from),
        });
        self.redo.clear();
    }

    pub fn pop_undo(&mut self) -> Option<Entry> {
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<Entry> {
        self.redo.pop()
    }

    pub fn push_undo(&mut self, entry: Entry) {
        self.undo.push(entry);
    }

    pub fn push_redo(&mut self, entry: Entry) {
        self.redo.push(entry);
    }

    pub fn unfinished(&mut self) -> Option<&mut Entry> {
        self.undo
            .iter_mut()
            .rev()
            .find(|entry| entry.left.is_some())
    }

    pub fn stacks(&self) -> (&[Entry], &[Entry]) {
        (&self.undo, &self.redo)
    }
}

impl Entry {
    /// A retry's undo runs before the undo of what the first run landed, like the app's.
    pub fn fold(&mut self, undo: Option<&Op>) {
        let Some(undo) = undo else {
            return;
        };
        let earlier = std::mem::replace(&mut self.undo, Step::Batch { ops: Vec::new() });
        let mut steps = Vec::new();
        for step in [Step::from(undo), earlier] {
            match step {
                Step::Batch { ops } => steps.extend(ops),
                step => steps.push(step),
            }
        }
        self.undo = Step::Batch { ops: steps };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(op: Op) -> String {
        let step = Step::from(&op);
        let text = serde_json::to_string(&step).unwrap();
        let back = Op::from(serde_json::from_str::<Step>(&text).unwrap());
        assert_eq!(format!("{op:?}"), format!("{back:?}"), "{text}");
        text
    }

    #[test]
    fn every_op_survives_the_journal() {
        let p = |name: &str| PathBuf::from(format!("/tmp/{name}"));
        round_trip(Op::Move {
            items: vec![p("a"), p("b")],
            into: p("dest"),
        });
        round_trip(Op::CopyExactly {
            items: vec![p("a")],
            into: p("dest"),
        });
        round_trip(Op::PutBack(vec![Restore {
            item: p(".Trash/a"),
            to: p("a"),
        }]));
        round_trip(Op::Retry(Request::each(
            Kind::Move,
            [(p("a"), p("dest/a"))],
            Conflict::Merge,
            OnError::Stop,
        )));
        round_trip(Op::Quick(Quick::Convert {
            items: vec![p("a.png")],
            format: ImageFormat::Heif,
            size: ImageSize::Small,
            metadata: true,
        }));
        let text = round_trip(Op::Batch(vec![
            Op::Trash(vec![p("a")]),
            Op::Rename {
                path: p("b"),
                name: "c".into(),
            },
        ]));
        assert!(text.contains(r#""op":"batch""#), "{text}");
    }

    #[test]
    fn a_retry_folds_its_undo_in_front() {
        let mut entry = Entry {
            label: "Move".into(),
            undo: Step::from(&Op::Trash(vec![PathBuf::from("/tmp/first")])),
            redo: Step::from(&Op::Trash(Vec::new())),
            serial: 1,
            conflict: Policy::KeepBoth,
            left: None,
        };
        entry.fold(Some(&Op::Trash(vec![PathBuf::from("/tmp/second")])));
        let Step::Batch { ops } = &entry.undo else {
            panic!("folded into a batch");
        };
        let firsts: Vec<_> = ops
            .iter()
            .map(|step| match step {
                Step::Trash { items } => items[0].clone(),
                _ => panic!("only trashes"),
            })
            .collect();
        assert_eq!(firsts, [PathBuf::from("/tmp/second"), "/tmp/first".into()]);
    }
}
