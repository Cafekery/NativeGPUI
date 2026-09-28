use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    let root = match mount == Path::new("/System/Volumes/Data") {
        true => PathBuf::from("/"),
        false => mount,
    };
    Some((root, records))
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let taken = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(taken)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn utf16(&mut self) -> Option<String> {
        let units = self.u32()? as usize;
        let raw = self.take(units.checked_mul(2)?)?;
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|unit| u16::from_be_bytes([unit[0], unit[1]]))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

fn put_backs(bytes: &[u8]) -> Option<Records> {
    let mut header = Reader::new(bytes);
    (header.u32()? == 1 && header.take(4)? == b"Bud1").then_some(())?;
    let bytes = &bytes[4..];
    let (offset, size) = (header.u32()? as usize, header.u32()? as usize);
    let mut root = Reader::new(bytes.get(offset..offset.checked_add(size)?)?);
    let count = root.u32()? as usize;
    root.u32()?;
    let addresses = (0..count).map(|_| root.u32()).collect::<Option<Vec<_>>>()?;
    root.take((count.next_multiple_of(256) - count) * 4)?;
    let mut dsdb = None;
    for _ in 0..root.u32()? {
        let length = root.take(1)?[0] as usize;
        let name = root.take(length)?;
        let id = root.u32()?;
        if name == b"DSDB" {
            dsdb = Some(id);
        }
    }
    let block = |id: u32| {
        let address = *addresses.get(id as usize)?;
        let offset = (address & !0x1f) as usize;
        let size = 1usize.checked_shl(address & 0x1f)?;
        Some(Reader::new(bytes.get(offset..offset.checked_add(size)?)?))
    };
    let mut db = block(dsdb?)?;
    let node = db.u32()?;
    let levels = db.u32().filter(|&levels| levels <= 32)?;
    db.u32()?;
    let mut budget = db.u32()?;
    let mut found = HashMap::new();
    walk(&block, node, levels, &mut budget, &mut found)?;
    Some(
        found
            .into_iter()
            .filter_map(|(name, (parent, file))| Some((name, (parent?, file?))))
            .collect(),
    )
}

type Found = HashMap<String, (Option<String>, Option<String>)>;

fn walk<'a>(
    block: &impl Fn(u32) -> Option<Reader<'a>>,
    id: u32,
    depth: u32,
    budget: &mut u32,
    found: &mut Found,
) -> Option<()> {
    *budget = budget.checked_sub(1)?;
    let mut node = block(id)?;
    let rightmost = node.u32()?;
    for _ in 0..node.u32()? {
        if rightmost != 0 {
            walk(block, node.u32()?, depth.checked_sub(1)?, budget, found)?;
        }
        record(&mut node, found)?;
    }
    if rightmost != 0 {
        walk(block, rightmost, depth.checked_sub(1)?, budget, found)?;
    }
    Some(())
}

fn record(node: &mut Reader, found: &mut Found) -> Option<()> {
    let name = node.utf16()?;
    let code = node.take(4)?;
    let text = match node.take(4)? {
        b"ustr" => Some(node.utf16()?),
        kind => {
            let length = match kind {
                b"bool" => 1,
                b"long" | b"shor" | b"type" => 4,
                b"comp" | b"dutc" => 8,
                b"blob" => node.u32()? as usize,
                _ => return None,
            };
            node.take(length)?;
            None
        }
    };
    match code {
        b"ptbL" => found.entry(name).or_default().0 = text,
        b"ptbN" => found.entry(name).or_default().1 = text,
        _ => {}
    }
    Some(())
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
