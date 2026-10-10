use std::path::Path;

use finder_fs::{comment, tags};
use objc2::AnyThread as _;
use objc2_app_kit::{NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
use objc2_foundation::{NSNumber, NSString, NSURL, NSURLIsUserImmutableKey};
use serde_json::json;

use crate::args::Args;
use crate::out::{self, Out};
use crate::{OK, Stop};

pub fn tag(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let action = args.shift().unwrap_or_default();
    let name = match action.as_str() {
        "list" => None,
        "add" | "remove" => Some(
            args.shift()
                .ok_or_else(|| Stop::Usage(format!("tag {action} needs a tag name")))?,
        ),
        _ => return Err(Stop::Usage("tag takes list, add or remove".into())),
    };
    args.done("tag")?;
    let paths = args.paths();
    if paths.is_empty() {
        return Err(Stop::Usage(format!("tag {action} needs a path")));
    }
    for path in &paths {
        std::fs::symlink_metadata(path)
            .map_err(|error| Stop::Refused(format!("{}: {error}", path.display())))?;
    }
    let changed = match (action.as_str(), name) {
        ("add", Some(name)) => {
            let color = (tags::all().into_iter())
                .find(|known| known.name == name)
                .map_or(0, |known| known.color);
            tags::apply(&paths, &[tags::Tag { name, color }], &[])
        }
        ("remove", Some(name)) => tags::apply(&paths, &[], &[name]),
        _ => Ok(()),
    };
    changed.map_err(|error| Stop::Failed(error.to_string()))?;
    for path in &paths {
        let held = tags::of(path);
        match out.json {
            true => out.emit(json!({
                "event": "tags",
                "path": out::path(path),
                "tags": held.iter().map(|tag| json!({ "name": tag.name, "color": tag.color })).collect::<Vec<_>>(),
            })),
            false => {
                let names: Vec<&str> = held.iter().map(|tag| tag.name.as_str()).collect();
                out.say(&format!("{}: {}", path.display(), names.join(", ")));
            }
        }
    }
    Ok(OK)
}

pub fn comment(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let clear = args.flag("clear");
    args.done("comment")?;
    let path = args
        .shift_path()
        .ok_or_else(|| Stop::Usage("comment needs a path".into()))?;
    let text = args.rest();
    std::fs::symlink_metadata(&path)
        .map_err(|error| Stop::Refused(format!("{}: {error}", path.display())))?;
    let new = match (clear, text.as_slice()) {
        (true, []) => Some(String::new()),
        (false, []) => None,
        (false, words) => Some(
            (words.iter())
                .map(|word| word.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        (true, _) => return Err(Stop::Usage("--clear takes no comment".into())),
    };
    if let Some(new) = new {
        comment::set(&path, &new).map_err(|error| Stop::Failed(error.to_string()))?;
    }
    let held = comment::of(&path);
    match out.json {
        true => out.emit(json!({ "event": "comment", "path": out::path(&path), "comment": held })),
        false => out.say(&held),
    }
    Ok(OK)
}

/// What Get Info's Locked checkbox sets.
pub fn lock(mut args: Args, on: bool, out: &mut Out) -> Result<u8, Stop> {
    let command = if on { "lock" } else { "unlock" };
    args.done(command)?;
    let paths = args.paths();
    if paths.is_empty() {
        return Err(Stop::Usage(format!("{command} needs a path")));
    }
    let mut failed = 0;
    for path in &paths {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let set = unsafe {
            url.setResourceValue_forKey_error(
                Some(&NSNumber::new_bool(on)),
                NSURLIsUserImmutableKey,
            )
        };
        if let Err(error) = set {
            failed += 1;
            out.warn(&format!(
                "{}: {}",
                path.display(),
                error.localizedDescription()
            ));
            continue;
        }
        if out.json {
            out.emit(json!({ "event": command, "path": out::path(path), "locked": finder_fs::ops::locked(path) }));
        }
    }
    Ok(outcome(failed, paths.len()))
}

pub fn icon(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let action = args.shift().unwrap_or_default();
    args.done("icon")?;
    let (image, paths) = match action.as_str() {
        "set" => {
            let path = args.shift_path();
            let image = args.shift_path();
            let (Some(path), Some(image)) = (path, image) else {
                return Err(Stop::Usage("icon set needs a path and an image".into()));
            };
            let loaded = NSImage::initWithContentsOfFile(
                NSImage::alloc(),
                &NSString::from_str(&image.to_string_lossy()),
            )
            .ok_or_else(|| Stop::Refused(format!("{} is not an image", image.display())))?;
            (Some(loaded), vec![path])
        }
        "remove" => (None, args.paths()),
        _ => return Err(Stop::Usage("icon takes set or remove".into())),
    };
    if paths.is_empty() {
        return Err(Stop::Usage("icon remove needs a path".into()));
    }
    let mut failed = 0;
    for path in &paths {
        if !finder_fs::custom_icon::can_change(path) {
            failed += 1;
            out.warn(&format!("{}: its icon can't be changed", path.display()));
            continue;
        }
        if !set_icon(path, image.as_deref()) {
            failed += 1;
            out.warn(&format!("{}: the icon could not be set", path.display()));
            continue;
        }
        if out.json {
            out.emit(json!({
                "event": "icon",
                "path": out::path(path),
                "custom": finder_fs::custom_icon::stored(path).is_some(),
            }));
        }
    }
    Ok(outcome(failed, paths.len()))
}

fn set_icon(path: &Path, image: Option<&NSImage>) -> bool {
    NSWorkspace::sharedWorkspace().setIcon_forFile_options(
        image,
        &NSString::from_str(&path.to_string_lossy()),
        NSWorkspaceIconCreationOptions::empty(),
    )
}

fn outcome(failed: usize, of: usize) -> u8 {
    match failed {
        0 => OK,
        n if n == of => crate::REFUSED,
        _ => crate::PARTIAL,
    }
}
