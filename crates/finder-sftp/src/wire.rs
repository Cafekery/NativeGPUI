use crate::{Attrs, Entry, Error};

pub(crate) const INIT: u8 = 1;
pub(crate) const VERSION: u8 = 2;
pub(crate) const OPEN: u8 = 3;
pub(crate) const CLOSE: u8 = 4;
pub(crate) const READ: u8 = 5;
pub(crate) const WRITE: u8 = 6;
pub(crate) const LSTAT: u8 = 7;
pub(crate) const SETSTAT: u8 = 9;
pub(crate) const OPENDIR: u8 = 11;
pub(crate) const READDIR: u8 = 12;
pub(crate) const REMOVE: u8 = 13;
pub(crate) const MKDIR: u8 = 14;
pub(crate) const RMDIR: u8 = 15;
pub(crate) const REALPATH: u8 = 16;
pub(crate) const STAT: u8 = 17;
pub(crate) const RENAME: u8 = 18;
pub(crate) const READLINK: u8 = 19;
pub(crate) const SYMLINK: u8 = 20;
pub(crate) const EXTENDED: u8 = 200;

const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;
const EXTENDED_REPLY: u8 = 201;

pub(crate) const FXF_READ: u32 = 0x01;
pub(crate) const FXF_WRITE: u32 = 0x02;
pub(crate) const FXF_CREAT: u32 = 0x08;
pub(crate) const FXF_TRUNC: u32 = 0x10;

pub(crate) const FX_EOF: u32 = 1;
pub(crate) const FX_NO_SUCH_FILE: u32 = 2;
pub(crate) const FX_PERMISSION_DENIED: u32 = 3;

const ATTR_SIZE: u32 = 0x1;
const ATTR_UIDGID: u32 = 0x2;
const ATTR_PERMISSIONS: u32 = 0x4;
const ATTR_ACMODTIME: u32 = 0x8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// Length, type and request id are filled in by `Session::send`.
const HEADER: usize = 9;

pub(crate) struct Packet(pub(crate) Vec<u8>);

impl Packet {
    pub(crate) fn new(kind: u8) -> Packet {
        let mut bytes = vec![0; HEADER];
        bytes[4] = kind;
        Packet(bytes)
    }

    pub(crate) fn u32(mut self, value: u32) -> Packet {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn u64(mut self, value: u64) -> Packet {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn bytes(self, value: &[u8]) -> Packet {
        let mut packet = self.u32(value.len() as u32);
        packet.0.extend_from_slice(value);
        packet
    }

    pub(crate) fn str(self, value: &str) -> Packet {
        self.bytes(value.as_bytes())
    }

    pub(crate) fn attrs(self, attrs: &Attrs) -> Packet {
        let mut flags = 0;
        let mut fields = Packet(Vec::new());
        if let Some(size) = attrs.size {
            flags |= ATTR_SIZE;
            fields = fields.u64(size);
        }
        if let (Some(uid), Some(gid)) = (attrs.uid, attrs.gid) {
            flags |= ATTR_UIDGID;
            fields = fields.u32(uid).u32(gid);
        }
        if let Some(permissions) = attrs.permissions {
            flags |= ATTR_PERMISSIONS;
            fields = fields.u32(permissions);
        }
        if let Some(mtime) = attrs.mtime {
            flags |= ATTR_ACMODTIME;
            fields = fields.u32(attrs.atime.unwrap_or(mtime)).u32(mtime);
        }
        let mut packet = self.u32(flags);
        packet.0.extend_from_slice(&fields.0);
        packet
    }
}

pub(crate) struct Cursor<'a>(pub(crate) &'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        if self.0.len() < len {
            return Err(Error::Protocol("truncated packet".into()));
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(head)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub(crate) fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    pub(crate) fn string(&mut self) -> Result<String, Error> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    pub(crate) fn attrs(&mut self) -> Result<Attrs, Error> {
        let flags = self.u32()?;
        let mut attrs = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            attrs.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            attrs.uid = Some(self.u32()?);
            attrs.gid = Some(self.u32()?);
        }
        if flags & ATTR_PERMISSIONS != 0 {
            attrs.permissions = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            attrs.atime = Some(self.u32()?);
            attrs.mtime = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(attrs)
    }
}

pub(crate) struct Reply {
    pub(crate) kind: u8,
    pub(crate) body: Vec<u8>,
}

impl Reply {
    fn expect(&self, kind: u8) -> Result<Cursor<'_>, Error> {
        if self.kind == kind {
            return Ok(Cursor(&self.body));
        }
        Err(self
            .ok()
            .err()
            .unwrap_or_else(|| Error::Protocol(format!("expected reply {kind}, got OK"))))
    }

    pub(crate) fn ok(&self) -> Result<(), Error> {
        if self.kind != STATUS {
            return Err(Error::Protocol(format!("unexpected reply {}", self.kind)));
        }
        let mut cursor = Cursor(&self.body);
        match cursor.u32()? {
            0 => Ok(()),
            code => Err(Error::Status {
                code,
                message: cursor.string().unwrap_or_default(),
            }),
        }
    }

    pub(crate) fn handle(&self) -> Result<Vec<u8>, Error> {
        Ok(self.expect(HANDLE)?.bytes()?.to_vec())
    }

    pub(crate) fn data(&self) -> Result<&[u8], Error> {
        self.expect(DATA)?.bytes()
    }

    pub(crate) fn attrs(&self) -> Result<Attrs, Error> {
        self.expect(ATTRS)?.attrs()
    }

    pub(crate) fn names(&self) -> Result<Vec<Entry>, Error> {
        let mut cursor = self.expect(NAME)?;
        (0..cursor.u32()?)
            .map(|_| {
                Ok(Entry {
                    name: cursor.string()?,
                    longname: cursor.string()?,
                    attrs: cursor.attrs()?,
                })
            })
            .collect()
    }

    pub(crate) fn name(&self) -> Result<String, Error> {
        self.names()?
            .into_iter()
            .next()
            .map(|entry| entry.name)
            .ok_or_else(|| Error::Protocol("empty NAME reply".into()))
    }

    pub(crate) fn extended(&self) -> Result<Cursor<'_>, Error> {
        self.expect(EXTENDED_REPLY)
    }
}
