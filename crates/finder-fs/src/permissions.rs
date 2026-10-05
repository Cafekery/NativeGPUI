use std::ffi::{CStr, c_char, c_int, c_void};
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
const ACL_FIRST_ENTRY: c_int = 0;
const ACL_NEXT_ENTRY: c_int = -1;
const ACL_EXTENDED_ALLOW: c_int = 1;
const ACL_READ_DATA: c_int = 1 << 1;
const ACL_WRITE_DATA: c_int = 1 << 2;
const ID_TYPE_GID: c_int = 1;
const SF_RESTRICTED: u32 = 0x0008_0000;

unsafe extern "C" {
    fn acl_get_file(path: *const c_char, kind: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, at: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    fn acl_get_qualifier(entry: *mut c_void) -> *mut c_void;
    fn acl_get_permset(entry: *mut c_void, permset: *mut *mut c_void) -> c_int;
    fn acl_get_perm_np(permset: *mut c_void, perm: c_int) -> c_int;
    fn acl_free(object: *mut c_void) -> c_int;
    fn mbr_uuid_to_id(uuid: *const u8, id: *mut libc::id_t, kind: *mut c_int) -> c_int;
}

/// A row's choice in Get Info's Sharing & Permissions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Privilege {
    ReadWrite,
    ReadOnly,
    WriteOnly,
    NoAccess,
    Custom,
}

impl Privilege {
    fn of(read: bool, write: bool) -> Self {
        match (read, write) {
            (true, true) => Self::ReadWrite,
            (true, false) => Self::ReadOnly,
            (false, true) => Self::WriteOnly,
            (false, false) => Self::NoAccess,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Class {
    Owner,
    Group,
    Everyone,
}

impl Class {
    fn shift(self) -> u32 {
        match self {
            Class::Owner => 6,
            Class::Group => 3,
            Class::Everyone => 0,
        }
    }
}

/// One row: the item's owner, an ACL entry, its group, or everyone. ACL entries have no class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub name: String,
    pub group: bool,
    pub me: bool,
    pub privilege: Privilege,
    pub class: Option<Class>,
}

/// The rows in Finder's order: owner, the ACL's allow entries, group, everyone.
pub fn grants(path: &Path) -> io::Result<Vec<Grant>> {
    let meta = std::fs::metadata(path)?;
    let me = unsafe { libc::getuid() };
    let posix = |class: Class, name: String, group: bool| {
        let bits = meta.mode() >> class.shift();
        Grant {
            me: !group && meta.uid() == me,
            name,
            group,
            privilege: Privilege::of(bits & 4 != 0, bits & 2 != 0),
            class: Some(class),
        }
    };
    let mut grants = vec![posix(Class::Owner, user_name(meta.uid()), false)];
    grants.extend(acl_grants(path, me));
    grants.push(posix(Class::Group, group_name(meta.gid()), true));
    grants.push(posix(Class::Everyone, "everyone".to_owned(), true));
    Ok(grants)
}

/// What you may do with the item yourself. Finder calls access to a SIP-restricted item custom.
pub fn yours(path: &Path) -> Privilege {
    use std::os::macos::fs::MetadataExt as _;
    if std::fs::metadata(path).is_ok_and(|meta| meta.st_flags() & SF_RESTRICTED != 0) {
        return Privilege::Custom;
    }
    let Ok(path) = crate::c_path(path) else {
        return Privilege::NoAccess;
    };
    let can = |mode| unsafe { libc::access(path.as_ptr(), mode) } == 0;
    Privilege::of(can(libc::R_OK), can(libc::W_OK))
}

pub fn mode(path: &Path) -> io::Result<u32> {
    Ok(std::fs::metadata(path)?.mode() & 0o7777)
}

pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Gives `class` the privilege, which only the owner (or root) may do.
pub fn set(path: &Path, class: Class, privilege: Privilege) -> io::Result<()> {
    let meta = std::fs::metadata(path)?;
    set_mode(path, with(meta.mode(), meta.is_dir(), class, privilege))
}

/// A folder's search bit follows read or write, as Finder sets it; a file keeps its own.
fn with(mode: u32, folder: bool, class: Class, privilege: Privilege) -> u32 {
    let (read, write) = match privilege {
        Privilege::ReadWrite => (true, true),
        Privilege::ReadOnly => (true, false),
        Privilege::WriteOnly => (false, true),
        Privilege::NoAccess => (false, false),
        Privilege::Custom => return mode & 0o7777,
    };
    let shift = class.shift();
    let search = match folder {
        true => read || write,
        false => mode >> shift & 1 != 0,
    };
    let bits = u32::from(read) << 2 | u32::from(write) << 1 | u32::from(search);
    mode & 0o7777 & !(7 << shift) | bits << shift
}

/// "Apply to enclosed items…" without root: every enclosed item you own takes the folder's
/// privileges, deepest first so a folder losing your access is still walked. Returns how many
/// changed.
pub fn apply_to_enclosed(folder: &Path) -> io::Result<usize> {
    let from = std::fs::metadata(folder)?.mode();
    let me = unsafe { libc::getuid() };
    let mut owned: Vec<(PathBuf, u32, bool)> = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                pending.push(entry.path());
            }
            if meta.uid() == me && !meta.file_type().is_symlink() {
                owned.push((entry.path(), meta.mode(), meta.is_dir()));
            }
        }
    }
    let mut changed = 0;
    for (path, mode, is_dir) in owned.into_iter().rev() {
        let wanted = match is_dir {
            true => mode & 0o7000 | from & 0o777,
            false => mode & 0o7111 | from & 0o666,
        };
        if wanted != mode & 0o7777 && set_mode(&path, wanted).is_ok() {
            changed += 1;
        }
    }
    Ok(changed)
}

/// Finder calls root "system".
fn user_name(uid: libc::uid_t) -> String {
    if uid == 0 {
        return "system".to_owned();
    }
    let user = unsafe { libc::getpwuid(uid) };
    name_or_unknown(match user.is_null() {
        true => std::ptr::null(),
        false => unsafe { (*user).pw_name },
    })
}

fn group_name(gid: libc::gid_t) -> String {
    let group = unsafe { libc::getgrgid(gid) };
    name_or_unknown(match group.is_null() {
        true => std::ptr::null(),
        false => unsafe { (*group).gr_name },
    })
}

fn name_or_unknown(name: *const c_char) -> String {
    match name.is_null() {
        true => "(unknown)".to_owned(),
        false => unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned(),
    }
}

/// Deny entries, such as the home folder's "everyone deny delete", grant nothing and get no row.
fn acl_grants(path: &Path, me: libc::uid_t) -> Vec<Grant> {
    let Ok(path) = crate::c_path(path) else {
        return Vec::new();
    };
    let acl = unsafe { acl_get_file(path.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        return Vec::new();
    }
    let mut grants = Vec::new();
    let mut entry = std::ptr::null_mut();
    let mut at = ACL_FIRST_ENTRY;
    while unsafe { acl_get_entry(acl, at, &mut entry) } == 0 {
        at = ACL_NEXT_ENTRY;
        let mut tag = 0;
        let mut permset = std::ptr::null_mut();
        if unsafe { acl_get_tag_type(entry, &mut tag) } != 0
            || tag != ACL_EXTENDED_ALLOW
            || unsafe { acl_get_permset(entry, &mut permset) } != 0
        {
            continue;
        }
        let has = |perm| unsafe { acl_get_perm_np(permset, perm) } == 1;
        let privilege = match (has(ACL_READ_DATA), has(ACL_WRITE_DATA)) {
            (false, false) => Privilege::Custom,
            (read, write) => Privilege::of(read, write),
        };
        let uuid = unsafe { acl_get_qualifier(entry) };
        if uuid.is_null() {
            continue;
        }
        let (mut id, mut kind) = (0, 0);
        let known = unsafe { mbr_uuid_to_id(uuid.cast(), &mut id, &mut kind) } == 0;
        unsafe { acl_free(uuid) };
        if !known {
            continue;
        }
        let group = kind == ID_TYPE_GID;
        grants.push(Grant {
            name: match group {
                true => group_name(id),
                false => user_name(id),
            },
            group,
            me: !group && id == me,
            privilege,
            class: None,
        });
    }
    unsafe { acl_free(acl) };
    grants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_searches_what_it_can_read_or_write() {
        assert_eq!(with(0o755, true, Class::Group, Privilege::WriteOnly), 0o735);
        assert_eq!(
            with(0o755, true, Class::Everyone, Privilege::NoAccess),
            0o750
        );
        assert_eq!(with(0o700, true, Class::Group, Privilege::ReadOnly), 0o750);
        assert_eq!(
            with(0o4755, false, Class::Owner, Privilege::ReadOnly),
            0o4555
        );
        assert_eq!(
            with(0o644, false, Class::Everyone, Privilege::ReadWrite),
            0o646
        );
        assert_eq!(with(0o640, false, Class::Group, Privilege::Custom), 0o640);
    }

    #[test]
    fn rows_follow_the_mode_and_apply_reaches_every_enclosed_item() {
        let root =
            std::env::temp_dir().join(format!("finder-fs-permissions-{}", std::process::id()));
        let inner = root.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(inner.join("b.sh"), b"b").unwrap();
        set_mode(&inner.join("b.sh"), 0o755).unwrap();
        set_mode(&root, 0o755).unwrap();

        let rows = grants(&root).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].me);
        assert_eq!(rows[0].class, Some(Class::Owner));
        assert_eq!(rows[2].name, "everyone");
        assert_eq!(rows[1].privilege, Privilege::ReadOnly);
        assert_eq!(yours(&root), Privilege::ReadWrite);

        set(&root, Class::Group, Privilege::ReadWrite).unwrap();
        set(&root, Class::Everyone, Privilege::NoAccess).unwrap();
        assert_eq!(mode(&root).unwrap(), 0o770);
        assert_eq!(grants(&root).unwrap()[1].privilege, Privilege::ReadWrite);

        assert_eq!(apply_to_enclosed(&root).unwrap(), 3);
        assert_eq!(mode(&inner).unwrap(), 0o770);
        assert_eq!(mode(&root.join("a.txt")).unwrap(), 0o660);
        assert_eq!(mode(&inner.join("b.sh")).unwrap(), 0o771);
        assert_eq!(apply_to_enclosed(&root).unwrap(), 0);

        std::fs::remove_dir_all(&root).unwrap();
    }
}
