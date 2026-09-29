use std::path::Path;
use std::sync::{Arc, LazyLock};

use finder_fs::meta::Sharer;
use gpui::SharedString;

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Extra {
    pub(crate) opened: Option<f64>,
    pub(crate) tags: Vec<(SharedString, u8)>,
    pub(crate) version: Option<SharedString>,
    pub(crate) comment: SharedString,
    pub(crate) shared_by: Option<Sharer>,
}

static ME: LazyLock<String> = LazyLock::new(|| finder_fs::words::finder("CS196", "Me"));

impl Extra {
    pub(crate) fn sharer_words(&self) -> Option<&str> {
        self.shared_by.as_ref().map(|sharer| match sharer {
            Sharer::Me => ME.as_str(),
            Sharer::Named(name) => name,
        })
    }

    /// Finder's sharing columns say `--` for no one.
    pub(crate) fn sharer_cell(&self) -> SharedString {
        match &self.shared_by {
            Some(Sharer::Me) => SharedString::new_static(ME.as_str()),
            Some(Sharer::Named(name)) => name.clone().into(),
            None => SharedString::new_static("--"),
        }
    }
}

pub(crate) fn extra_of(path: &str, package: bool, comment: Option<String>) -> Arc<Extra> {
    let at = Path::new(path);
    Arc::new(Extra {
        opened: finder_fs::meta::last_used(at),
        tags: finder_fs::tags::stored(at)
            .into_iter()
            .map(|tag| (tag.name.into(), tag.color))
            .collect(),
        version: package
            .then(|| finder_fs::meta::bundle_version(at))
            .flatten()
            .map(Into::into),
        comment: comment.map(Into::into).unwrap_or_default(),
        shared_by: None,
    })
}
