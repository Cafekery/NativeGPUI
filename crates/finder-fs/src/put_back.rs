use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ds_store::{self, Value};
use crate::volumes::mount_point;

type Records = HashMap<String, (String, String)>;

pub fn origins(items: &[PathBuf]) -> Vec<Option<PathBuf>> {
    let mut trashes: HashMap<&Path, Option<(PathBuf, Records)>> = HashMap::new();
    items
        .iter()
        .map(|item| {
            let folder = item.parent()?;
            let (root, records) = trashes
                .entry(folder)
                .or_insert_with(|| trash(folder))
                .as_ref()?;
            let (parent, name) = records.get(item.file_name()?.to_str()?)?;
            let parent = parent.trim_start_matches('/');
            let parent = parent
                .strip_prefix("System/Volumes/Data/")
                .filter(|rest| !rest.is_empty())
                .unwrap_or(parent);
            Some(root.join(parent).join(name))
        })
        .collect()
}

fn trash(folder: &Path) -> Option<(PathBuf, Records)> {
    let records = put_backs(&std::fs::read(folder.join(".DS_Store")).ok()?)?;
    let mount = mount_point(folder)?;
    let root = if mount == Path::new("/System/Volumes/Data") {
        PathBuf::from("/")
    } else {
        mount
    };
    Some((root, records))
}

fn put_backs(bytes: &[u8]) -> Option<Records> {
    let mut found: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for record in ds_store::records(bytes)? {
        let Value::Text(text) = record.value else {
            continue;
        };
        match &record.code {
            b"ptbL" => found.entry(record.name).or_default().0 = Some(text),
            b"ptbN" => found.entry(record.name).or_default().1 = Some(text),
            _ => {}
        }
    }
    Some(
        found
            .into_iter()
            .filter_map(|(name, (parent, file))| Some((name, (parent?, file?))))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_trashed_items_original_folder_and_name() {
        let bytes = include_bytes!("testdata/trash.DS_Store");
        let expected: Records = [
            ("alpha.txt", "/Plans/", "alpha.txt"),
            ("alpha.txt 01-34-51-493.txt", "/Plans/Q3/", "alpha.txt"),
            ("beta.md", "/Plans/Q3/", "beta.md"),
            ("gamma", "/", "gamma"),
        ]
        .into_iter()
        .map(|(name, parent, file)| (name.into(), (parent.into(), file.into())))
        .collect();
        assert_eq!(put_backs(bytes), Some(expected));
        for end in 0..bytes.len() {
            assert_eq!(put_backs(&bytes[..end]), None);
        }
    }
}
