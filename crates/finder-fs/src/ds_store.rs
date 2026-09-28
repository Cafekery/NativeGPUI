use std::collections::HashMap;
use std::path::Path;

use objc2_foundation::{
    NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization, NSString,
};

/// What one record of a `.DS_Store` holds.
#[derive(Debug, PartialEq)]
pub enum Value<'a> {
    Long(u32),
    Short(u32),
    Bool(bool),
    Type([u8; 4]),
    Comp(u64),
    Dutc(u64),
    Blob(&'a [u8]),
    Text(String),
}

/// One fact Finder keeps about the item `name` (`.` for the folder itself).
#[derive(Debug, PartialEq)]
pub struct Record<'a> {
    pub name: String,
    pub code: [u8; 4],
    pub value: Value<'a>,
}

/// Every record of a `.DS_Store`, or `None` if any part of it cannot be read.
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

/// What Finder recorded about a folder shown as icons.
#[derive(Debug, Default, PartialEq)]
pub struct IconView {
    /// Finder's `arrangeBy`: `none`, `grid`, or the key it keeps the icons sorted by.
    pub arrange_by: Option<String>,
    /// Where Finder last put each icon: the centre of its image, in points from the top left.
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
            arrange_by(&records(&bytes)?, name.to_str()?)
        });
    IconView {
        arrange_by: above.or_else(|| arrange_by(&own, ".")),
        locations: icon_locations(&own),
    }
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

fn arrange_by(records: &[Record], name: &str) -> Option<String> {
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
    let arrange = options.objectForKey(&NSString::from_str("arrangeBy"))?;
    Some(arrange.downcast::<NSString>().ok()?.to_string())
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

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    fn code(&mut self) -> Option<[u8; 4]> {
        self.take(4)?.try_into().ok()
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
        b"long" => Value::Long(node.u32()?),
        b"shor" => Value::Short(node.u32()?),
        b"bool" => Value::Bool(node.take(1)?[0] != 0),
        b"type" => Value::Type(node.code()?),
        b"comp" => Value::Comp(node.u64()?),
        b"dutc" => Value::Dutc(node.u64()?),
        b"blob" => {
            let length = node.u32()? as usize;
            Value::Blob(node.take(length)?)
        }
        b"ustr" => Value::Text(node.utf16()?),
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
}
