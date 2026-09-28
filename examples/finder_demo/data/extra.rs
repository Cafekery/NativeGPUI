use std::path::Path;
use std::sync::Arc;

use gpui::SharedString;

/// What a row shows that its listing does not say: each field is its own file's.
#[derive(Default, PartialEq)]
pub(crate) struct Extra {
    pub(crate) opened: Option<f64>,
    pub(crate) tags: Vec<(SharedString, u8)>,
    pub(crate) version: Option<SharedString>,
}

/// Background work only: every field is a read of the item's own file.
pub(crate) fn extra_of(path: &str, package: bool) -> Arc<Extra> {
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
    })
}
