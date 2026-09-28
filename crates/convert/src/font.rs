use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::{Error, Format, Intent, Job, Outline, PathTaken, Planned, Probe, Report, Result};

pub fn sniff(data: &[u8]) -> Option<Format> {
    match data.get(..4)? {
        b"wOFF" => Some(Format::Woff),
        b"wOF2" => Some(Format::Woff2),
        b"OTTO" => Some(Format::Otf),
        b"\x00\x01\x00\x00" | b"true" => Some(Format::Ttf),
        _ => None,
    }
}

pub fn is_collection(path: &Path, data: &[u8]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ttc") || e.eq_ignore_ascii_case("otc"))
        || data.starts_with(b"ttcf")
}

pub fn collection_error() -> Error {
    Error::new("font collections (.ttc) can’t be converted — one face at a time")
}

pub fn outline_of(sfnt: &[u8]) -> Result<Outline> {
    let cff = table_records(sfnt)?
        .iter()
        .any(|t| t.tag == *b"CFF " || t.tag == *b"CFF2");
    Ok(if cff { Outline::Cff } else { Outline::TrueType })
}

pub fn to_sfnt(data: Vec<u8>, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Ttf | Format::Otf => Ok(data),
        Format::Woff => wuff::decompress_woff1(&data).map_err(Error::new),
        Format::Woff2 => wuff::decompress_woff2(&data).map_err(Error::new),
        _ => Err(Error::new("not a font")),
    }
}

pub fn run(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    let source = std::fs::read(&job.source)?;
    let bytes = match planned.path {
        PathTaken::Copy => source,
        PathTaken::Remux => {
            let sfnt = to_sfnt(source, probe.format)?;
            let outline = probe.outline.map_or_else(|| outline_of(&sfnt), Ok)?;
            match job.target {
                Format::Ttf | Format::Otf => sfnt,
                Format::Woff => wrap_woff1(&sfnt)?,
                Format::Woff2 => wrap_woff2(&sfnt, outline, job.intent)?,
                _ => return Err(Error::new("not a font target")),
            }
        }
        PathTaken::Encode => return Err(Error::new("fonts are wrapped, not redrawn")),
    };
    std::fs::write(&job.dest, &bytes)?;
    Ok(Report::new(
        probe,
        planned.path,
        bytes.len() as u64,
        planned.notes.clone(),
    ))
}

fn wrap_woff2(sfnt: &[u8], outline: Outline, intent: Intent) -> Result<Vec<u8>> {
    let quality = ttf2woff2::BrotliQuality::from(if intent == Intent::Web { 9 } else { 11 });
    if outline == Outline::TrueType {
        if let Ok(bytes) = ttf2woff2::encode(sfnt, quality) {
            return Ok(bytes);
        }
    }
    ttf2woff2::encode_no_transform(sfnt, quality).map_err(Error::new)
}

fn wrap_woff1(sfnt: &[u8]) -> Result<Vec<u8>> {
    let tables = table_records(sfnt)?;
    let dir_end = 44 + tables.len() * 20;
    let mut dir = Vec::with_capacity(tables.len() * 20);
    let mut body = Vec::new();
    for table in &tables {
        body.resize(body.len().next_multiple_of(4), 0);
        let orig = table.bytes(sfnt)?;
        let comp = zlib_compress(orig)?;
        let stored = if comp.len() < orig.len() {
            &comp[..]
        } else {
            orig
        };
        dir.extend_from_slice(&table.tag);
        dir.extend_from_slice(&((dir_end + body.len()) as u32).to_be_bytes());
        dir.extend_from_slice(&(stored.len() as u32).to_be_bytes());
        dir.extend_from_slice(&table.length.to_be_bytes());
        dir.extend_from_slice(&table.checksum.to_be_bytes());
        body.extend_from_slice(stored);
    }

    let total = dir_end + body.len();
    let (major, minor) = head_version(sfnt, &tables);
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&read_u32(sfnt, 0)?.to_be_bytes());
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
    out.extend_from_slice(&major.to_be_bytes());
    out.extend_from_slice(&minor.to_be_bytes());
    out.extend_from_slice(&[0; 20]);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    Ok(out)
}

struct TableRec {
    tag: [u8; 4],
    checksum: u32,
    offset: u32,
    length: u32,
}

impl TableRec {
    fn bytes<'a>(&self, sfnt: &'a [u8]) -> Result<&'a [u8]> {
        let start = self.offset as usize;
        start
            .checked_add(self.length as usize)
            .and_then(|end| sfnt.get(start..end))
            .ok_or_else(truncated)
    }
}

fn table_records(sfnt: &[u8]) -> Result<Vec<TableRec>> {
    (0..read_u16(sfnt, 4)? as usize)
        .map(|i| {
            let o = 12 + i * 16;
            Ok(TableRec {
                tag: be_bytes(sfnt, o)?,
                checksum: read_u32(sfnt, o + 4)?,
                offset: read_u32(sfnt, o + 8)?,
                length: read_u32(sfnt, o + 12)?,
            })
        })
        .collect()
}

fn head_version(sfnt: &[u8], tables: &[TableRec]) -> (u16, u16) {
    tables
        .iter()
        .find(|t| t.tag == *b"head")
        .and_then(|head| head.bytes(sfnt).ok())
        .and_then(|head| Some((read_u16(head, 4).ok()?, read_u16(head, 6).ok()?)))
        .unwrap_or((1, 0))
}

fn zlib_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

fn truncated() -> Error {
    Error::new("truncated font")
}

fn be_bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    data.get(offset..offset + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(truncated)
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    be_bytes(data, offset).map(u16::from_be_bytes)
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    be_bytes(data, offset).map(u32::from_be_bytes)
}

#[cfg(test)]
pub fn table_bytes(sfnt: &[u8], tag: &[u8; 4]) -> Result<Vec<u8>> {
    let tables = table_records(sfnt)?;
    let table = tables
        .iter()
        .find(|t| t.tag == *tag)
        .ok_or_else(|| Error::new("missing font table"))?;
    Ok(table.bytes(sfnt)?.to_vec())
}
