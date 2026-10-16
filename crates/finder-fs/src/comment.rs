use std::ffi::CStr;
use std::io;
use std::path::Path;

use objc2_foundation::{NSPropertyListFormat, NSPropertyListSerialization, NSString};

use crate::{ds_store, xattr};

const COMMENT: &CStr = c"com.apple.metadata:kMDItemFinderComment";

/// Finder shows only the `.DS_Store` record; the attribute alone reads as blank.
pub fn of(path: &Path) -> String {
    let recorded = || ds_store::comments(path.parent()?).remove(path.file_name()?.to_str()?);
    recorded().unwrap_or_default()
}

pub fn set(path: &Path, comment: &str) -> io::Result<()> {
    let (Some(folder), Some(name)) = (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
    ) else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    let item = crate::c_path(path)?;
    if comment.is_empty() {
        xattr::remove(&item, COMMENT)?;
    } else {
        xattr::set(&item, COMMENT, &binary_plist(comment)?)?;
    }
    ds_store::set_comment(folder, name, comment)
}

fn binary_plist(comment: &str) -> io::Result<Vec<u8>> {
    unsafe {
        NSPropertyListSerialization::dataWithPropertyList_format_options_error(
            &NSString::from_str(comment),
            NSPropertyListFormat::BinaryFormat_v1_0,
            0,
        )
    }
    .map(|data| data.to_vec())
    .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn a_comment_lands_in_the_attribute_and_the_store_and_leaves_both() {
        let dir = std::env::temp_dir().join(format!("finder-fs-comment-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Note.txt");
        std::fs::write(&path, "").unwrap();
        let item = crate::c_path(&path).unwrap();
        set(&path, "a note ✓").unwrap();
        let read = of(&path);
        let attribute = xattr::get(&item, COMMENT)
            .and_then(|bytes| crate::plist(&bytes))
            .and_then(|object| Some(object.downcast::<NSString>().ok()?.to_string()));
        let stored = ds_store::comments(&dir);
        set(&path, "").unwrap();
        let cleared = (
            of(&path),
            xattr::get(&item, COMMENT),
            ds_store::comments(&dir),
        );
        set(&path, "a note ✓").unwrap();
        std::fs::remove_file(dir.join(".DS_Store")).unwrap();
        let attribute_alone = of(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(read, "a note ✓");
        assert_eq!(attribute.as_deref(), Some("a note ✓"));
        assert_eq!(stored.get("Note.txt").map(String::as_str), Some("a note ✓"));
        assert_eq!(attribute_alone, "");
        assert_eq!(cleared, (String::new(), None, HashMap::new()));
    }
}
