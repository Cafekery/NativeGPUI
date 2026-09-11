use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::format::Outline;
use crate::plan::{Intent, PathTaken, Planned};
use crate::report::Report;
use crate::{Error, Format, Job, Probe, Result};

pub fn sniff(data: &[u8]) -> Option<Format> {
    if data.len() < 4 {
        return None;
    }
    match &data[..4] {
        b"wOFF" => Some(Format::Woff),
        b"wOF2" => Some(Format::Woff2),
        b"OTTO" => Some(Format::Otf),
        b"\x00\x01\x00\x00" | b"true" => Some(Format::Ttf),
        _ => None,
    }
}

pub fn is_collection(path: &Path, data: &[u8]) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("ttc" | "otc")) || data.starts_with(b"ttcf")
}

pub fn collection_error() -> Error {
    Error::new("font collections (.ttc) can’t be converted — one face at a time")
}

pub fn outline_of(sfnt: &[u8]) -> Result<Outline> {
    let tables = table_records(sfnt)?;
    if tables
        .iter()
        .any(|t| t.tag == *b"CFF " || t.tag == *b"CFF2")
    {
        Ok(Outline::Cff)
    } else {
        Ok(Outline::TrueType)
    }
}

pub fn to_sfnt(data: &[u8], format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Ttf | Format::Otf => Ok(data.to_vec()),
        Format::Woff => wuff::decompress_woff1(data).map_err(|e| Error::new(e.to_string())),
        Format::Woff2 => wuff::decompress_woff2(data).map_err(|e| Error::new(e.to_string())),
        _ => Err(Error::new("not a font")),
    }
}

pub fn run(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    match planned.path {
        PathTaken::Copy => write_bytes(
            job,
            probe,
            planned,
            &std::fs::read(&job.source).map_err(|e| Error::new(e.to_string()))?,
        ),
        PathTaken::Remux => {
            let src = std::fs::read(&job.source).map_err(|e| Error::new(e.to_string()))?;
            let sfnt = to_sfnt(&src, probe.format)?;
            let outline = match probe.outline {
                Some(o) => o,
                None => outline_of(&sfnt)?,
            };
            refuse_cff_as_ttf(outline, job.target)?;
            let out = to_target(&sfnt, outline, job.target, job.intent)?;
            write_bytes(job, probe, planned, &out)
        }
        PathTaken::Encode => Err(Error::new("fonts are wrapped, not redrawn")),
    }
}

fn refuse_cff_as_ttf(outline: Outline, target: Format) -> Result<()> {
    if target == Format::Ttf && outline == Outline::Cff {
        return Err(Error::new(
            "This font is drawn as OpenType (CFF). Saving as TTF would redraw every letter. Save as OTF to install it.",
        ));
    }
    Ok(())
}

fn to_target(sfnt: &[u8], outline: Outline, target: Format, intent: Intent) -> Result<Vec<u8>> {
    match target {
        Format::Ttf | Format::Otf => Ok(sfnt.to_vec()),
        Format::Woff => wrap_woff1(sfnt),
        Format::Woff2 => wrap_woff2(sfnt, outline, intent),
        _ => Err(Error::new("not a font target")),
    }
}

fn wrap_woff2(sfnt: &[u8], outline: Outline, intent: Intent) -> Result<Vec<u8>> {
    let quality = ttf2woff2::BrotliQuality::from(match intent {
        Intent::Web => 9u8,
        _ => 11u8,
    });
    let first = if outline == Outline::Cff {
        ttf2woff2::encode_no_transform(sfnt, quality)
    } else {
        ttf2woff2::encode(sfnt, quality)
    };
    match first {
        Ok(bytes) => Ok(bytes),
        Err(_) if outline != Outline::Cff => {
            ttf2woff2::encode_no_transform(sfnt, quality).map_err(|e| Error::new(e.to_string()))
        }
        Err(e) => Err(Error::new(e.to_string())),
    }
}

fn wrap_woff1(sfnt: &[u8]) -> Result<Vec<u8>> {
    let tables = table_records(sfnt)?;
    let flavor = read_u32(sfnt, 0)?;
    let mut packed = Vec::with_capacity(tables.len());
    for rec in &tables {
        let orig = rec.bytes(sfnt)?;
        let comp = zlib_compress(orig)?;
        let stored = if comp.len() < orig.len() {
            comp
        } else {
            orig.to_vec()
        };
        packed.push(Packed {
            tag: rec.tag,
            checksum: rec.checksum,
            orig_len: rec.length,
            stored,
        });
    }

    let dir_end = 44 + packed.len() * 20;
    let mut offset = dir_end as u32;
    let mut body = Vec::new();
    let mut dir = Vec::with_capacity(packed.len() * 20);
    for table in &packed {
        while body.len() % 4 != 0 {
            body.push(0);
            offset += 1;
        }
        dir.extend_from_slice(&table.tag);
        dir.extend_from_slice(&offset.to_be_bytes());
        dir.extend_from_slice(&(table.stored.len() as u32).to_be_bytes());
        dir.extend_from_slice(&table.orig_len.to_be_bytes());
        dir.extend_from_slice(&table.checksum.to_be_bytes());
        body.extend_from_slice(&table.stored);
        offset += table.stored.len() as u32;
    }

    let total = (dir_end + body.len()) as u32;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&(packed.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
    let (major, minor) = head_version(sfnt, &tables);
    out.extend_from_slice(&major.to_be_bytes());
    out.extend_from_slice(&minor.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    Ok(out)
}

struct Packed {
    tag: [u8; 4],
    checksum: u32,
    orig_len: u32,
    stored: Vec<u8>,
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
        let end = start
            .checked_add(self.length as usize)
            .ok_or_else(|| Error::new("truncated font"))?;
        sfnt.get(start..end)
            .ok_or_else(|| Error::new("truncated font"))
    }
}

fn table_records(sfnt: &[u8]) -> Result<Vec<TableRec>> {
    let num = read_u16(sfnt, 4)? as usize;
    let mut out = Vec::with_capacity(num);
    for i in 0..num {
        let o = 12 + i * 16;
        let tag = sfnt
            .get(o..o + 4)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| Error::new("truncated font"))?;
        out.push(TableRec {
            tag,
            checksum: read_u32(sfnt, o + 4)?,
            offset: read_u32(sfnt, o + 8)?,
            length: read_u32(sfnt, o + 12)?,
        });
    }
    Ok(out)
}

fn head_version(sfnt: &[u8], tables: &[TableRec]) -> (u16, u16) {
    let Some(head) = tables.iter().find(|t| t.tag == *b"head") else {
        return (1, 0);
    };
    let Ok(bytes) = head.bytes(sfnt) else {
        return (1, 0);
    };
    if bytes.len() < 8 {
        return (1, 0);
    }
    (
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
    )
}

fn zlib_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(data).map_err(|e| Error::new(e.to_string()))?;
    enc.finish().map_err(|e| Error::new(e.to_string()))
}

fn write_bytes(job: &Job, probe: &Probe, planned: &Planned, bytes: &[u8]) -> Result<Report> {
    std::fs::write(&job.dest, bytes).map_err(|e| Error::new(e.to_string()))?;
    Ok(Report {
        bytes_in: probe.bytes,
        bytes_out: bytes.len() as u64,
        width_in: None,
        height_in: None,
        width_out: None,
        height_out: None,
        duration_in: None,
        duration_out: None,
        path: planned.path,
        notes: planned.notes.clone(),
    })
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| Error::new("truncated font"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| Error::new("truncated font"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
pub fn table_bytes(sfnt: &[u8], tag: &[u8; 4]) -> Result<Vec<u8>> {
    let tables = table_records(sfnt)?;
    let rec = tables
        .iter()
        .find(|t| t.tag == *tag)
        .ok_or_else(|| Error::new("missing font table"))?;
    Ok(rec.bytes(sfnt)?.to_vec())
}
