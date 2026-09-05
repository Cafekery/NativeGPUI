//! What Finder calls a file: its uniform type identifier and the localized kind string.
//!
//! Contract: [`kind_of`] is cheap enough to call per row while sorting 20 000 entries — cache
//! per extension. The real implementation asks UniformTypeIdentifiers (`UTType`) and Launch
//! Services for the localized description; the placeholder is a small table.

use std::sync::Arc;

use crate::entry::{Entry, EntryKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    /// e.g. `public.plain-text`, `com.apple.application-bundle`, `public.folder`.
    pub uti: Arc<str>,
    /// Finder's localized label, e.g. `Plain Text`, `Application`, `Folder`.
    pub label: Arc<str>,
}

pub fn kind_of(entry: &Entry) -> Kind {
    match entry.kind {
        EntryKind::Directory if !entry.package => Kind {
            uti: "public.folder".into(),
            label: "Folder".into(),
        },
        EntryKind::Symlink => Kind {
            uti: "public.symlink".into(),
            label: "Alias".into(),
        },
        _ => kind_for_extension(entry.extension().unwrap_or("")),
    }
}

pub fn kind_for_extension(ext: &str) -> Kind {
    let (uti, label) = match ext.to_ascii_lowercase().as_str() {
        "app" => ("com.apple.application-bundle", "Application"),
        "pdf" => ("com.adobe.pdf", "PDF Document"),
        "txt" | "md" => ("public.plain-text", "Plain Text"),
        "png" => ("public.png", "PNG image"),
        "jpg" | "jpeg" => ("public.jpeg", "JPEG image"),
        "zip" => ("public.zip-archive", "ZIP archive"),
        "dmg" => ("com.apple.disk-image-udif", "Disk Image"),
        "mov" => ("com.apple.quicktime-movie", "QuickTime Movie"),
        "" => ("public.data", "Document"),
        other => {
            return Kind {
                uti: "public.data".into(),
                label: format!("{} File", other.to_ascii_uppercase()).into(),
            };
        }
    };
    Kind {
        uti: uti.into(),
        label: label.into(),
    }
}

/// Whether a directory with this name is a package Finder shows as a file.
pub fn is_package_name(name: &str) -> bool {
    matches!(
        name.rsplit_once('.').map(|(_, ext)| ext),
        Some("app" | "bundle" | "framework" | "kext" | "plugin" | "xcodeproj" | "photoslibrary")
    )
}
