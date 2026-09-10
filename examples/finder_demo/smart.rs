//! Smart folders: a search saved the way Finder saves one, in ~/Library/Saved Searches, and run
//! again through Spotlight each time it opens.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use finder_fs::index::Index;
use gpui::{Context, Window};
use objc2_foundation::{NSNumber, NSString, NSURL, NSURLHasHiddenExtensionKey};

use crate::app::FinderDemo;
use crate::chrome::criteria_bar::Criterion;
use crate::chrome::scope_bar;
use crate::data::{self, Row, intern, intern_path};

/// Stands where a `tag:` id would, for the listing a saved search opens as.
const PREFIX: &str = "search:";
/// Finder's codes for a search of the whole Mac, and of the folders listed with it.
const THIS_MAC: u32 = 1396925814;
const FOLDERS: u32 = 1396925286;

pub(crate) fn is_saved(path: &str) -> bool {
    path.ends_with(".savedSearch")
}

/// The listing `id` opens as, when it is a saved search.
pub(crate) fn listing(id: &str) -> Option<&'static str> {
    is_saved(id).then(|| intern(&format!("{PREFIX}{id}")))
}

/// The saved search a listing reads, when it is one.
pub(crate) fn file_of(listing: &str) -> Option<&str> {
    listing.strip_prefix(PREFIX)
}

/// Finder's Spotlight query for the search field and every criteria row, all of them held; with
/// nothing to hold, Finder saves `false`.
fn raw_query(text: &str, criteria: &[Criterion]) -> String {
    let text = text.trim();
    let clauses: Vec<String> = (!text.is_empty())
        .then(|| name_clause("*", text, "*"))
        .into_iter()
        .chain(criteria.iter().filter_map(Criterion::raw_query))
        .collect();
    match clauses.is_empty() {
        true => "false".to_string(),
        false => clauses.join(" && "),
    }
}

pub(crate) fn name_clause(before: &str, text: &str, after: &str) -> String {
    format!("(kMDItemFSName == \"{before}{}{after}\"cd)", escape(text))
}

/// Spotlight reads `*` in a string as a wildcard, so a typed one is escaped with the quote and the
/// backslash.
pub(crate) fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '"' | '*') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Written where Finder keeps its own, under `name`; no `folders` means the whole Mac.
fn save(name: &str, raw: &str, folders: &[String]) -> Result<PathBuf, String> {
    let dir = crate::home().join("Library/Saved Searches");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    // Finder shows a colon on disk as a slash, and so takes a slash in a name as one.
    let file = dir.join(format!("{}.savedSearch", name.replace('/', ":")));
    if file.exists() {
        return Err(format!(
            "The name \u{201c}{name}\u{201d} is already taken. Please choose a different name."
        ));
    }
    std::fs::write(&file, plist(raw, folders)).map_err(|error| error.to_string())?;
    // Finder hides a smart folder's extension, and titles its window by that; failing only shows it.
    let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
    let yes = NSNumber::new_bool(true);
    let _ = unsafe { url.setResourceValue_forKey_error(Some(&yes), NSURLHasHiddenExtensionKey) };
    Ok(file)
}

/// The keys Finder itself writes, its raw-query criteria row included so Finder can edit the
/// search too.
fn plist(raw: &str, folders: &[String]) -> String {
    let string = |text: &str| format!("<string>{}</string>", xml(text));
    let (code, scopes, paths) = match folders.is_empty() {
        true => {
            let everywhere = string("kMDQueryScopeComputer");
            (THIS_MAC, everywhere.clone(), everywhere)
        }
        false => (
            FOLDERS,
            folders
                .iter()
                .map(|folder| format!("<array>{}</array>", string(folder)))
                .collect(),
            folders.iter().map(|folder| string(folder)).collect(),
        ),
    };
    let raw = string(raw);
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\"><dict>",
            "<key>CompatibleVersion</key><integer>1</integer>",
            "<key>RawQuery</key>{raw}",
            "<key>RawQueryDict</key><dict>",
            "<key>FinderFilesOnly</key><false/>",
            "<key>RawQuery</key>{raw}",
            "<key>SearchScopes</key><array>{scopes}</array>",
            "<key>UserFilesOnly</key><true/>",
            "</dict>",
            "<key>SearchCriteria</key><dict>",
            "<key>FXCriteriaSlices</key><array><dict>",
            "<key>criteria</key><array><string>com_apple_RawQueryAttribute</string>",
            "<integer>104</integer></array>",
            "<key>displayValues</key><array><string>Raw query</string>{raw}</array>",
            "<key>rowType</key><integer>0</integer><key>subrows</key><array/>",
            "</dict></array>",
            "<key>FXScope</key><integer>{code}</integer>",
            "<key>FXScopeArrayOfPaths</key><array>{paths}</array>",
            "</dict>",
            "</dict></plist>\n",
        ),
        raw = raw,
        scopes = scopes,
        code = code,
        paths = paths,
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unxml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// What a saved search finds now: its query run through Spotlight in each folder it names.
pub(crate) fn rows(file: &str, index: &Index, limit: usize) -> Vec<Arc<Row>> {
    let file = Path::new(file);
    let Some(raw) = plutil(file, "RawQuery", "raw") else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for root in roots(file) {
        let Ok(out) = Command::new("/usr/bin/mdfind")
            .arg("-onlyin")
            .arg(&root)
            .arg(&raw)
            .output()
        else {
            continue;
        };
        found.extend(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_owned),
        );
    }
    found.sort();
    found.dedup();
    data::rows_at(found.iter().take(limit).map(String::as_str), index)
}

/// The folders a saved search looks in; Finder names the whole Mac and the home folder by constant.
fn roots(file: &Path) -> Vec<PathBuf> {
    let listed = plutil(file, "SearchCriteria.FXScopeArrayOfPaths", "xml1").unwrap_or_default();
    let roots: Vec<PathBuf> = listed
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("<string>")?
                .strip_suffix("</string>")
        })
        .map(|scope| match scope {
            "kMDQueryScopeComputer" | "kMDQueryScopeComputerIndexed" => PathBuf::from("/"),
            "kMDQueryScopeHome" | "kMDQueryScopeHomeIndexed" | "kMDQueryScopeMyFiles" => {
                crate::home()
            }
            path => PathBuf::from(unxml(path)),
        })
        .collect();
    match roots.is_empty() {
        true => vec![PathBuf::from("/")],
        false => roots,
    }
}

fn plutil(file: &Path, key: &str, format: &str) -> Option<String> {
    let out = Command::new("/usr/bin/plutil")
        .args(["-extract", key, format, "-o", "-"])
        .arg(file)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

impl FinderDemo {
    /// Finder's Save for a search: the file, its row in the sidebar, then the search shown as it.
    pub(crate) fn save_search(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let folders: Vec<String> = match self.scope {
            scope_bar::THIS_MAC => Vec::new(),
            scope_bar::NETWORK => data::network_roots()
                .iter()
                .map(|root| root.to_string())
                .collect(),
            _ => vec![self.here().to_string()],
        };
        match save(name, &raw_query(&self.query, &self.criteria), &folders) {
            Ok(file) => {
                self.pin_favourite(vec![file.clone()]);
                self.go(intern_path(&file), window, cx);
            }
            Err(error) => self.complain(&error),
        }
    }
}
