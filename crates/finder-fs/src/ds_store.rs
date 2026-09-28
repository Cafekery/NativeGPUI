use std::collections::HashMap;
use std::path::Path;

use objc2_foundation::{
    NSData, NSDictionary, NSNumber, NSPropertyListMutabilityOptions, NSPropertyListSerialization,
    NSString,
};

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
pub struct IconOptions {
    pub arrange_by: Option<String>,
    pub icon_size: Option<f32>,
    pub grid_spacing: Option<f32>,
    pub text_size: Option<f32>,
    pub label_on_bottom: Option<bool>,
}

impl IconOptions {
    pub fn read(options: &NSDictionary) -> Self {
        let value = |key: &str| options.objectForKey(&NSString::from_str(key));
        let number = |key: &str| value(key)?.downcast::<NSNumber>().ok();
        Self {
            arrange_by: value("arrangeBy")
                .and_then(|text| Some(text.downcast::<NSString>().ok()?.to_string())),
            icon_size: number("iconSize").map(|size| size.floatValue()),
            grid_spacing: number("gridSpacing").map(|spacing| spacing.floatValue()),
            text_size: number("textSize").map(|size| size.floatValue()),
            label_on_bottom: number("labelOnBottom").map(|bottom| bottom.boolValue()),
        }
    }

    pub fn or(self, fallback: Self) -> Self {
        Self {
            arrange_by: self.arrange_by.or(fallback.arrange_by),
            icon_size: self.icon_size.or(fallback.icon_size),
            grid_spacing: self.grid_spacing.or(fallback.grid_spacing),
            text_size: self.text_size.or(fallback.text_size),
            label_on_bottom: self.label_on_bottom.or(fallback.label_on_bottom),
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

fn icon_locations(records: &[Record]) -> HashMap<String, (f32, f32)> {
    records
        .iter()
        .filter_map(|record| {
            let Value::Blob(blob) = record.value else {
                return None;
            };
            (&record.code == b"Iloc").then_some(())?;
            let mut blob = Reader::new(blob);
            let x = blob.u32()? as i32;
            let y = blob.u32()? as i32;
            Some((record.name.clone(), (x as f32, y as f32)))
        })
        .collect()
}

fn icon_options(records: &[Record], name: &str) -> Option<IconOptions> {
    let blob = records.iter().find_map(|record| match record.value {
        Value::Blob(blob) if record.name == name && &record.code == b"icvp" => Some(blob),
        _ => None,
    })?;
    let options = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(blob),
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .ok()?
    .downcast::<NSDictionary>()
    .ok()?;
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
    found: &mut Vec<Record<'a>>,
) -> Option<()> {
    *budget = budget.checked_sub(1)?;
    let mut node = block(id)?;
    let rightmost = node.u32()?;
    for _ in 0..node.u32()? {
        if rightmost != 0 {
            walk(block, node.u32()?, depth.checked_sub(1)?, budget, found)?;
        }
        found.push(record(&mut node)?);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::env::temp_dir().join(format!("finder-fs-comments-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = include_bytes!("testdata/comments.DS_Store");
        std::fs::write(dir.join(".DS_Store"), store).unwrap();
        let expected: HashMap<String, String> = [
            ("Banana.txt", "beta note"),
            ("Cherry.txt", "cmmt only"),
            ("Date.txt", "alpha note"),
        ]
        .into_iter()
        .map(|(name, comment)| (name.to_owned(), comment.to_owned()))
        .collect();
        let found = comments(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, expected);
    }
}
