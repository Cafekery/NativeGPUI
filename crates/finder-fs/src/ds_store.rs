use std::cmp::Ordering;
use std::collections::HashMap;
use std::io;
use std::path::Path;

use objc2_foundation::{NSDictionary, NSNumber, NSString};

#[derive(Debug, PartialEq)]
pub enum Value<'a> {
    Blob(&'a [u8]),
    Text(String),
    Other,
}

/// One fact Finder keeps about the item `name` (`.` for the folder itself).
#[derive(Debug, PartialEq)]
pub struct Record<'a> {
    pub name: String,
    pub code: [u8; 4],
    pub value: Value<'a>,
}

pub fn records(bytes: &[u8]) -> Option<Vec<Record<'_>>> {
    entries(bytes).map(|found| found.into_iter().map(|(record, _)| record).collect())
}

fn entries(bytes: &[u8]) -> Option<Vec<(Record<'_>, &[u8])>> {
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
    let mut found = Vec::new();
    walk(&block, node, levels, &mut budget, &mut found)?;
    Some(found)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Background {
    #[default]
    Default,
    Color([f32; 3]),
    Picture(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconOptions {
    pub arrange_by: Option<String>,
    pub icon_size: Option<f32>,
    pub grid_spacing: Option<f32>,
    pub text_size: Option<f32>,
    pub label_on_bottom: Option<bool>,
    pub show_item_info: Option<bool>,
    pub show_icon_preview: Option<bool>,
    pub background: Option<Background>,
}

impl IconOptions {
    pub fn read(options: &NSDictionary) -> Self {
        let value = |key: &str| options.objectForKey(&NSString::from_str(key));
        let number = |key: &str| value(key)?.downcast::<NSNumber>().ok();
        let float = |key: &str| number(key).map(|number| number.floatValue());
        let flag = |key: &str| number(key).map(|number| number.boolValue());
        let text = |key: &str| Some(value(key)?.downcast::<NSString>().ok()?.to_string());
        let part = |key: &str| float(key).unwrap_or(1.);
        Self {
            arrange_by: text("arrangeBy"),
            icon_size: float("iconSize"),
            grid_spacing: float("gridSpacing"),
            text_size: float("textSize"),
            label_on_bottom: flag("labelOnBottom"),
            show_item_info: flag("showItemInfo"),
            show_icon_preview: flag("showIconPreview"),
            background: number("backgroundType").and_then(|kind| match kind.integerValue() {
                0 => Some(Background::Default),
                1 => Some(Background::Color([
                    part("backgroundColorRed"),
                    part("backgroundColorGreen"),
                    part("backgroundColorBlue"),
                ])),
                2 => Some(Background::Picture(
                    text("backgroundImagePath").unwrap_or_default(),
                )),
                _ => None,
            }),
        }
    }

    pub fn or(self, fallback: Self) -> Self {
        Self {
            arrange_by: self.arrange_by.or(fallback.arrange_by),
            icon_size: self.icon_size.or(fallback.icon_size),
            grid_spacing: self.grid_spacing.or(fallback.grid_spacing),
            text_size: self.text_size.or(fallback.text_size),
            label_on_bottom: self.label_on_bottom.or(fallback.label_on_bottom),
            show_item_info: self.show_item_info.or(fallback.show_item_info),
            show_icon_preview: self.show_icon_preview.or(fallback.show_icon_preview),
            background: self.background.or(fallback.background),
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct IconView {
    pub options: IconOptions,
    /// The centre of each icon's image, in points from the folder's top left.
    pub locations: HashMap<String, (f32, f32)>,
}

/// A folder's options live in its parent's `.DS_Store`, a volume root's in its own under `.`.
pub fn icon_view(folder: &Path) -> IconView {
    let own = std::fs::read(folder.join(".DS_Store")).ok();
    let own = own.as_deref().and_then(records).unwrap_or_default();
    let above = folder
        .parent()
        .zip(folder.file_name())
        .and_then(|(parent, name)| {
            let bytes = std::fs::read(parent.join(".DS_Store")).ok()?;
            icon_options(&records(&bytes)?, name.to_str()?)
        });
    IconView {
        options: above
            .or_else(|| icon_options(&own, "."))
            .unwrap_or_default(),
        locations: icon_locations(&own),
    }
}

pub fn comments(folder: &Path) -> HashMap<String, String> {
    let bytes = std::fs::read(folder.join(".DS_Store")).unwrap_or_default();
    records(&bytes)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|record| match record.value {
            Value::Text(text) if &record.code == b"cmmt" => Some((record.name, text)),
            _ => None,
        })
        .collect()
}

pub fn set_comment(folder: &Path, name: &str, comment: &str) -> io::Result<()> {
    let path = folder.join(".DS_Store");
    let bytes = match std::fs::read(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        read => read?,
    };
    let parsed = entries(&bytes);
    let tail = bytes
        .get(20..36)
        .filter(|_| parsed.is_some())
        .unwrap_or(&FRESH_TAIL);
    let found = parsed.unwrap_or_default();
    let existing = found
        .iter()
        .position(|(record, _)| record.name == name && &record.code == b"cmmt");
    let mut raws: Vec<&[u8]> = found.iter().map(|(_, raw)| *raw).collect();
    let encoded = comment_record(name, comment);
    match (existing, comment.is_empty()) {
        (Some(at), true) => _ = raws.remove(at),
        (Some(at), false) => raws[at] = &encoded,
        (None, true) => return Ok(()),
        (None, false) => {
            let at = found
                .iter()
                .position(|(record, _)| order(&record.name, &record.code, name, b"cmmt").is_gt())
                .unwrap_or(found.len());
            raws.insert(at, &encoded);
        }
    }
    let file = arena(&raws, tail).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "a record outgrows a .DS_Store page",
        )
    })?;
    let temporary = folder.join(format!(".DS_Store.{}", std::process::id()));
    std::fs::write(&temporary, file)
        .and_then(|()| std::fs::rename(&temporary, &path))
        .inspect_err(|_| _ = std::fs::remove_file(&temporary))
}

fn icon_locations(records: &[Record]) -> HashMap<String, (f32, f32)> {
    records
        .iter()
        .filter_map(|record| match record.value {
            Value::Blob(blob) if &record.code == b"Iloc" => {
                let mut blob = Reader::new(blob);
                let (x, y) = (blob.u32()? as i32, blob.u32()? as i32);
                Some((record.name.clone(), (x as f32, y as f32)))
            }
            _ => None,
        })
        .collect()
}

fn icon_options(records: &[Record], name: &str) -> Option<IconOptions> {
    let blob = records.iter().find_map(|record| match record.value {
        Value::Blob(blob) if record.name == name && &record.code == b"icvp" => Some(blob),
        _ => None,
    })?;
    let options = crate::plist(blob)?.downcast::<NSDictionary>().ok()?;
    Some(IconOptions::read(&options))
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

    fn code(&mut self) -> Option<[u8; 4]> {
        self.take(4)?.try_into().ok()
    }

    fn utf16(&mut self) -> Option<String> {
        let units = self.u32()? as usize;
        let raw = self.take(units.checked_mul(2)?)?;
        let units = raw
            .chunks_exact(2)
            .map(|unit| u16::from_be_bytes([unit[0], unit[1]]));
        Some(
            char::decode_utf16(units)
                .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect(),
        )
    }
}

fn walk<'a>(
    block: &impl Fn(u32) -> Option<Reader<'a>>,
    id: u32,
    depth: u32,
    budget: &mut u32,
    found: &mut Vec<(Record<'a>, &'a [u8])>,
) -> Option<()> {
    *budget = budget.checked_sub(1)?;
    let mut node = block(id)?;
    let rightmost = node.u32()?;
    for _ in 0..node.u32()? {
        if rightmost != 0 {
            walk(block, node.u32()?, depth.checked_sub(1)?, budget, found)?;
        }
        let start = node.at;
        let record = record(&mut node)?;
        found.push((record, &node.bytes[start..node.at]));
    }
    if rightmost != 0 {
        walk(block, rightmost, depth.checked_sub(1)?, budget, found)?;
    }
    Some(())
}

fn record<'a>(node: &mut Reader<'a>) -> Option<Record<'a>> {
    let name = node.utf16()?;
    let code = node.code()?;
    let value = match &node.code()? {
        b"blob" => {
            let length = node.u32()? as usize;
            Value::Blob(node.take(length)?)
        }
        b"ustr" => Value::Text(node.utf16()?),
        b"bool" => node.take(1).map(|_| Value::Other)?,
        b"long" | b"shor" | b"type" => node.take(4).map(|_| Value::Other)?,
        b"comp" | b"dutc" => node.take(8).map(|_| Value::Other)?,
        _ => return None,
    };
    Some(Record { name, code, value })
}

const PAGE: usize = 0x1000;
const FRESH_TAIL: [u8; 16] = [0, 0, 1, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn order(name: &str, code: &[u8; 4], other: &str, other_code: &[u8; 4]) -> Ordering {
    fn fold(name: &str) -> impl Iterator<Item = char> + '_ {
        name.chars().flat_map(char::to_lowercase)
    }
    fold(name)
        .cmp(fold(other))
        .then_with(|| code.cmp(other_code))
}

fn put_utf16(out: &mut Vec<u8>, text: &str) {
    out.extend((text.encode_utf16().count() as u32).to_be_bytes());
    out.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
}

fn comment_record(name: &str, comment: &str) -> Vec<u8> {
    let mut record = Vec::new();
    put_utf16(&mut record, name);
    record.extend(b"cmmtustr");
    put_utf16(&mut record, comment);
    record
}

struct Buddy {
    free: [Vec<u32>; 32],
}

impl Buddy {
    fn new() -> Self {
        let mut free: [Vec<u32>; 32] = Default::default();
        free[31].push(0);
        Self { free }
    }

    fn alloc(&mut self, size: usize) -> Option<u32> {
        let want = size.max(32).checked_next_power_of_two()?.trailing_zeros() as usize;
        let mut k = (want..32).find(|&k| !self.free[k].is_empty())?;
        let offset = self.free[k].remove(0);
        while k > want {
            k -= 1;
            let buddy = offset + (1 << k);
            let at = self.free[k].partition_point(|&free| free < buddy);
            self.free[k].insert(at, buddy);
        }
        Some(offset | want as u32)
    }
}

struct Layout {
    buddy: Buddy,
    addresses: Vec<u32>,
    blocks: Vec<(u32, Vec<u8>)>,
}

impl Layout {
    fn block(&mut self, bytes: Vec<u8>) -> Option<u32> {
        let address = self.buddy.alloc(bytes.len())?;
        self.addresses.push(address);
        self.blocks.push((address, bytes));
        Some(self.addresses.len() as u32 - 1)
    }

    fn node(&mut self, records: &[&[u8]], children: &[u32]) -> Option<u32> {
        let mut bytes = Vec::new();
        bytes.extend(children.last().copied().unwrap_or(0).to_be_bytes());
        bytes.extend((records.len() as u32).to_be_bytes());
        for (at, record) in records.iter().enumerate() {
            if let Some(child) = children.get(at) {
                bytes.extend(child.to_be_bytes());
            }
            bytes.extend(*record);
        }
        self.block(bytes)
    }
}

fn arena(records: &[&[u8]], tail: &[u8]) -> Option<Vec<u8>> {
    records
        .iter()
        .all(|record| 12 + record.len() <= PAGE)
        .then_some(())?;
    let mut buddy = Buddy::new();
    buddy.alloc(32)?;
    let dsdb = buddy.alloc(20)?;
    let mut layout = Layout {
        buddy,
        addresses: vec![0, dsdb],
        blocks: Vec::new(),
    };
    let mut level = records.to_vec();
    let mut children = Vec::new();
    let mut levels = 0u32;
    let root = loop {
        let internal = !children.is_empty();
        let cost = |record: &[u8]| record.len() + if internal { 4 } else { 0 };
        if 8 + level.iter().map(|record| cost(record)).sum::<usize>() <= PAGE {
            break layout.node(&level, &children)?;
        }
        let (mut separators, mut parents, mut start, mut size) = (Vec::new(), Vec::new(), 0, 8);
        for (at, record) in level.iter().enumerate() {
            if size + cost(record) <= PAGE {
                size += cost(record);
                continue;
            }
            let separator = if at + 1 == level.len() { at - 1 } else { at };
            let below = children.get(start..=separator).unwrap_or_default();
            parents.push(layout.node(&level[start..separator], below)?);
            separators.push(level[separator]);
            start = separator + 1;
            size = 8 + level[start..=at]
                .iter()
                .map(|record| cost(record))
                .sum::<usize>();
        }
        let below = children.get(start..).unwrap_or_default();
        parents.push(layout.node(&level[start..], below)?);
        level = separators;
        children = parents;
        levels += 1;
    };
    let nodes = layout.blocks.len() as u32;
    let db = [root, levels, records.len() as u32, nodes, PAGE as u32]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
    layout.blocks.push((dsdb, db));
    let count = layout.addresses.len();
    let slots = count.next_multiple_of(256);
    let free_bound = layout.buddy.free.iter().map(Vec::len).sum::<usize>() + 32;
    let root_size = 8 + 4 * slots + 4 + 9 + 4 * 32 + 4 * free_bound;
    let root_block = layout.buddy.alloc(root_size)?;
    layout.addresses[0] = root_block;
    let mut info = Vec::new();
    info.extend((count as u32).to_be_bytes());
    info.extend(0u32.to_be_bytes());
    info.extend(
        (0..slots).flat_map(|at| layout.addresses.get(at).copied().unwrap_or(0).to_be_bytes()),
    );
    info.extend(1u32.to_be_bytes());
    info.extend(b"\x04DSDB");
    info.extend(1u32.to_be_bytes());
    for free in &layout.buddy.free {
        info.extend((free.len() as u32).to_be_bytes());
        info.extend(free.iter().flat_map(|offset| offset.to_be_bytes()));
    }
    layout.blocks.push((root_block, info));
    let span = |address: u32| ((address & !0x1f) as usize, 1usize << (address & 0x1f));
    let end = layout
        .blocks
        .iter()
        .map(|&(address, _)| span(address))
        .map(|(offset, size)| offset + size)
        .max()?;
    let (root_offset, root_size) = span(root_block);
    let root_at = (root_offset as u32).to_be_bytes();
    let mut file = [
        &1u32.to_be_bytes()[..],
        b"Bud1",
        &root_at,
        &(root_size as u32).to_be_bytes(),
        &root_at,
        tail,
    ]
    .concat();
    file.resize(4 + end, 0);
    for (address, bytes) in &layout.blocks {
        let offset = 4 + span(*address).0;
        file[offset..offset + bytes.len()].copy_from_slice(bytes);
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMENTS: &[u8] = include_bytes!("testdata/comments.DS_Store");

    #[test]
    fn reads_each_icon_centre_finder_recorded_on_and_off_the_grid() {
        let bytes = include_bytes!("testdata/icons.DS_Store");
        let expected: HashMap<String, (f32, f32)> = [
            ("alpha.txt", 66., 46.),
            ("Charlie", 290., 46.),
            ("delta.md", 402., 46.),
            ("foxtrot.txt", 66., 158.),
            ("golf.txt", 178., 158.),
            ("hotel.txt", 290., 158.),
            ("india.txt", 402., 158.),
            ("Juliet", 514., 158.),
            ("echo.txt", 420., 250.),
            ("kilo.txt", 66., 270.),
            ("bravo.txt", 178., 270.),
        ]
        .into_iter()
        .map(|(name, x, y)| (name.to_owned(), (x, y)))
        .collect();
        let found = records(bytes).expect("a .DS_Store Finder wrote");
        assert_eq!(icon_locations(&found), expected);
        for end in 0..bytes.len() {
            assert!(records(&bytes[..end]).is_none_or(|cut| cut == found));
        }
    }

    #[test]
    fn reads_the_comment_finder_set_on_each_item() {
        let dir = folder("comments", Some(COMMENTS));
        let found = comments(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            found,
            notes(&[
                ("Banana.txt", "beta note"),
                ("Cherry.txt", "cmmt only"),
                ("Date.txt", "alpha note"),
            ])
        );
    }

    fn folder(tag: &str, store: Option<&[u8]>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("finder-fs-{tag}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(store) = store {
            std::fs::write(dir.join(".DS_Store"), store).unwrap();
        }
        dir
    }

    fn notes(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, comment)| (name.to_string(), comment.to_string()))
            .collect()
    }

    fn sorted(records: &[Record]) -> bool {
        records
            .windows(2)
            .all(|pair| order(&pair[0].name, &pair[0].code, &pair[1].name, &pair[1].code).is_lt())
    }

    #[test]
    fn rewrites_a_comment_finder_set_and_keeps_the_others() {
        let dir = folder("rewrite", Some(COMMENTS));
        set_comment(&dir, "Banana.txt", "new").unwrap();
        let found = comments(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            found,
            notes(&[
                ("Banana.txt", "new"),
                ("Cherry.txt", "cmmt only"),
                ("Date.txt", "alpha note"),
            ])
        );
    }

    #[test]
    fn inserts_and_removes_comments_in_finder_order() {
        let dir = folder("insert", Some(COMMENTS));
        set_comment(&dir, "Apple.txt", "first").unwrap();
        set_comment(&dir, "banana 2.txt", "lower case").unwrap();
        set_comment(&dir, "Cherry.txt", "").unwrap();
        let found = comments(&dir);
        let bytes = std::fs::read(dir.join(".DS_Store")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            found,
            notes(&[
                ("Apple.txt", "first"),
                ("banana 2.txt", "lower case"),
                ("Banana.txt", "beta note"),
                ("Date.txt", "alpha note"),
            ])
        );
        let stored = records(&bytes).unwrap();
        let names: Vec<&str> = stored.iter().map(|record| record.name.as_str()).collect();
        assert_eq!(
            names,
            ["Apple.txt", "banana 2.txt", "Banana.txt", "Date.txt"]
        );
        assert!(sorted(&stored));
    }

    #[test]
    fn a_new_comment_leaves_every_icon_where_finder_put_it() {
        let store = include_bytes!("testdata/icons.DS_Store");
        let dir = folder("icons", Some(store));
        set_comment(&dir, "golf.txt", "still here").unwrap();
        let bytes = std::fs::read(dir.join(".DS_Store")).unwrap();
        let view = icon_view(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let before = records(store).unwrap();
        let after = records(&bytes).unwrap();
        assert!(sorted(&before) && sorted(&after));
        let added = Record {
            name: "golf.txt".into(),
            code: *b"cmmt",
            value: Value::Text("still here".into()),
        };
        assert_eq!(after.iter().filter(|record| **record == added).count(), 1);
        let kept: Vec<Record> = after
            .into_iter()
            .filter(|record| *record != added)
            .collect();
        assert_eq!(kept, before);
        assert_eq!(view.locations, icon_locations(&before));
    }

    #[test]
    fn creates_a_store_only_for_a_comment() {
        let dir = folder("fresh", None);
        set_comment(&dir, "a.txt", "").unwrap();
        let untouched = !dir.join(".DS_Store").exists();
        set_comment(&dir, "a.txt", "hello").unwrap();
        let bytes = std::fs::read(dir.join(".DS_Store")).unwrap();
        let files = std::fs::read_dir(&dir).unwrap().count();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(untouched);
        assert_eq!(files, 1);
        let expected = Record {
            name: "a.txt".into(),
            code: *b"cmmt",
            value: Value::Text("hello".into()),
        };
        assert_eq!(records(&bytes).unwrap(), [expected]);
    }

    #[test]
    fn keeps_every_comment_of_a_store_several_levels_deep() {
        let note = |index: usize| format!("{index:03} {}", "x".repeat(index % 7 * 150));
        let names: Vec<String> = (0..600)
            .map(|index| format!("item {index:03}.txt"))
            .collect();
        let encoded: Vec<Vec<u8>> = (0..600)
            .map(|index| comment_record(&names[index], &note(index)))
            .collect();
        let raws: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
        let store = arena(&raws, &FRESH_TAIL).unwrap();
        let levels = u32::from_be_bytes(store[0x28..0x2c].try_into().unwrap());
        let dir = folder("deep-store", Some(&store));
        let first = comments(&dir);
        set_comment(&dir, "item 300.txt", "changed").unwrap();
        let once = std::fs::read(dir.join(".DS_Store")).unwrap();
        set_comment(&dir, "item 300.txt", "changed").unwrap();
        let twice = std::fs::read(dir.join(".DS_Store")).unwrap();
        let second = comments(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(levels >= 2, "{levels} levels");
        let expected: HashMap<String, String> = (0..600)
            .map(|index| (names[index].clone(), note(index)))
            .collect();
        assert_eq!(first, expected);
        assert_eq!(second.len(), 600);
        assert_eq!(second["item 300.txt"], "changed");
        assert_eq!(once, twice);
        assert!(sorted(&records(&twice).unwrap()));
    }
}
