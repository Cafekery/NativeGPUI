//! The real file system as the demo sees it: interned absolute paths for ids, cached listings, and
//! the system formatters that turn entries into rows.

mod face;
mod folders;
mod format;
mod paths;
mod row;
mod volumes;

pub(crate) use self::face::Face;
pub(crate) use self::folders::{Folder, Folders, folder_of};
pub(crate) use self::format::{byte_string, date_text, day_string, group_of, stamp};
pub(crate) use self::paths::{enclosing_name, intern, intern_path, split_key};
pub(crate) use self::row::{
    Column, Line, Row, hit_row, icloud_rows, path_line, rows_of, tagged_rows,
};
pub(crate) use self::volumes::{
    VolumeInfo, available_bytes, refresh_volumes, volume_at, volume_roots,
};
