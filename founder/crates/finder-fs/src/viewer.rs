use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{
    CFPreferencesCopyValue, CFPreferencesSetValue, CFPreferencesSynchronize, CFString, CFURL,
    CFURLPathStyle, kCFPreferencesAnyApplication, kCFPreferencesAnyHost, kCFPreferencesCurrentUser,
};
#[allow(deprecated)]
use objc2_core_services::{LSCopyApplicationURLsForBundleIdentifier, LSRegisterURL};
use objc2_foundation::{NSDictionary, NSString, ns_string};

const KEY: &str = "NSFileViewer";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// A reinstall takes the bundle away for a moment, and Launch Services still knows a copy that moved.
const GUARD: &str = r#"present() { [ -e "$VIEWER_BUNDLE/Contents/Info.plist" ]; }
present && exit 0
sleep 2
present && exit 0
live() {
  /usr/bin/osascript -l JavaScript -e 'ObjC.import("AppKit")
function run(argv) {
  const urls = $.NSWorkspace.sharedWorkspace.URLsForApplicationsWithBundleIdentifier(argv[0])
  for (let i = 0; i < urls.count; i++) {
    const path = urls.objectAtIndex(i).path.js
    if (!/\/\.Trash(es)?\//.test(path) && $.NSFileManager.defaultManager.fileExistsAtPath(path)) return path
  }
  return ""
}' "$1" 2>/dev/null
}
[ -n "$(live "$VIEWER_APP")" ] && exit 0
if [ "$(/usr/bin/defaults read "$VIEWER_DOMAIN" NSFileViewer 2>/dev/null)" = "$VIEWER_APP" ]; then
  if [ -n "$VIEWER_PREVIOUS" ] && [ -n "$(live "$VIEWER_PREVIOUS")" ]; then
    /usr/bin/defaults write "$VIEWER_DOMAIN" NSFileViewer -string "$VIEWER_PREVIOUS"
  else
    /usr/bin/defaults delete "$VIEWER_DOMAIN" NSFileViewer
  fi
fi
/bin/rm -f "$VIEWER_PLIST"
/bin/launchctl bootout "gui/$(/usr/bin/id -u)/$VIEWER_LABEL"
"#;

pub struct Status {
    pub viewer: Option<String>,
    pub guard: Option<Guard>,
}

pub struct Guard {
    pub bundle: PathBuf,
    /// The viewer the guard puts back if it is still installed; `None` means Finder.
    pub previous: Option<String>,
    pub is_loaded: bool,
}

/// `NSFileViewer`; `None` means Finder.
pub fn current() -> Option<String> {
    viewer(None)
}

pub fn status(id: &str) -> io::Result<Status> {
    let site = Site::of(id)?;
    Ok(Status {
        viewer: viewer(site.domain.as_deref()),
        guard: site.guard(),
    })
}

/// Makes the app at `bundle` the viewer, guarded by a launch agent that hands the viewer back once the bundle is gone.
pub fn on(id: &str, bundle: &Path) -> io::Result<()> {
    Site::of(id)?.on(id, bundle)
}

pub fn off(id: &str) -> io::Result<()> {
    Site::of(id)?.off(id)
}

/// Points the guard at `bundle` while the app is the viewer, and drops a guard another viewer outlived.
pub fn follow(id: &str, bundle: &Path) -> io::Result<()> {
    Site::of(id)?.follow(id, bundle)
}

/// Launch Services opens an id with the copy it registered last; a stale build elsewhere otherwise opens as a second app.
pub fn register(bundle: &Path) {
    let path = CFString::from_str(&bundle.to_string_lossy());
    let url =
        CFURL::with_file_system_path(None, Some(&path), CFURLPathStyle::CFURLPOSIXPathStyle, true);
    if let Some(url) = url {
        unsafe { LSRegisterURL(&url, true) };
    }
}

/// Makes Launch Services forget every copy carrying one of `ids` other than `own`; blocks on `lsregister`.
pub fn unregister_others(ids: &[&str], own: &Path) {
    let own = canonical(own);
    for id in ids {
        for path in app_paths(id) {
            if canonical(&path) == own {
                continue;
            }
            // Launch Services has no public call to unregister a bundle.
            _ = Command::new(LSREGISTER)
                .arg("-u")
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// A translocated app runs from a read-only copy that vanishes when it quits.
pub fn is_translocated(bundle: &Path) -> bool {
    bundle
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
}

struct Site {
    /// `None` is the global domain.
    domain: Option<String>,
    label: String,
    plist: PathBuf,
}

impl Site {
    fn of(id: &str) -> io::Result<Site> {
        let home = crate::home()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        let label = format!("{id}.viewer-guard");
        Ok(Site {
            domain: None,
            plist: home.join(format!("Library/LaunchAgents/{label}.plist")),
            label,
        })
    }

    fn on(&self, id: &str, bundle: &Path) -> io::Result<()> {
        if is_translocated(bundle) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "a translocated app has no lasting path to guard",
            ));
        }
        let previous = match viewer(self.domain.as_deref()) {
            Some(viewer) if viewer == id => self.previous(),
            other => other,
        };
        self.install(&self.job(id, bundle, previous.as_deref())?)?;
        set_viewer(self.domain.as_deref(), Some(id)).inspect_err(|_| _ = self.remove())
    }

    fn off(&self, id: &str) -> io::Result<()> {
        if viewer(self.domain.as_deref()).as_deref() == Some(id) {
            let previous = self.previous().filter(|previous| is_installed(previous));
            set_viewer(self.domain.as_deref(), previous.as_deref())?;
        }
        self.remove()
    }

    fn follow(&self, id: &str, bundle: &Path) -> io::Result<()> {
        if viewer(self.domain.as_deref()).as_deref() != Some(id) {
            return if self.plist.exists() {
                self.remove()
            } else {
                Ok(())
            };
        }
        if is_translocated(bundle) {
            return Ok(());
        }
        let job = self.job(id, bundle, self.previous().as_deref())?;
        if std::fs::read_to_string(&self.plist).is_ok_and(|written| written == job)
            && self.is_loaded()
        {
            return Ok(());
        }
        self.install(&job)
    }

    fn guard(&self) -> Option<Guard> {
        let env = self.env()?;
        Some(Guard {
            bundle: PathBuf::from(var(&env, ns_string!("VIEWER_BUNDLE"))?),
            previous: var(&env, ns_string!("VIEWER_PREVIOUS")),
            is_loaded: self.is_loaded(),
        })
    }

    fn previous(&self) -> Option<String> {
        let env = self.env()?;
        var(&env, ns_string!("VIEWER_PREVIOUS"))
    }

    fn env(&self) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
        let job = crate::plist_dict(&std::fs::read(&self.plist).ok()?)?;
        let env = job
            .objectForKey(ns_string!("EnvironmentVariables"))?
            .downcast::<NSDictionary>()
            .ok()?;
        // SAFETY: property-list dictionary keys are always strings.
        Some(unsafe { Retained::cast_unchecked(env) })
    }

    fn job(&self, id: &str, bundle: &Path, previous: Option<&str>) -> io::Result<String> {
        let text = |path: &Path| {
            path.to_str()
                .map(escape)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path is not UTF-8"))
        };
        let (bundle, plist) = (text(bundle)?, text(&self.plist)?);
        let env = [
            ("VIEWER_APP", escape(id)),
            ("VIEWER_BUNDLE", bundle.clone()),
            (
                "VIEWER_DOMAIN",
                escape(self.domain.as_deref().unwrap_or("-g")),
            ),
            ("VIEWER_LABEL", escape(&self.label)),
            ("VIEWER_PLIST", plist),
            ("VIEWER_PREVIOUS", escape(previous.unwrap_or(""))),
        ]
        .map(|(key, value)| format!("\t\t<key>{key}</key>\n\t\t<string>{value}</string>\n"))
        .concat();
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/sh</string>
		<string>-c</string>
		<string>{script}</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
{env}	</dict>
	<key>WatchPaths</key>
	<array>
		<string>{bundle}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>StartInterval</key>
	<integer>3600</integer>
	<key>ThrottleInterval</key>
	<integer>2</integer>
</dict>
</plist>
"#,
            label = escape(&self.label),
            script = escape(GUARD),
        ))
    }

    fn install(&self, job: &str) -> io::Result<()> {
        if let Some(agents) = self.plist.parent() {
            std::fs::create_dir_all(agents)?;
        }
        std::fs::write(&self.plist, job)?;
        self.unload();
        launchctl(&[
            OsStr::new("bootstrap"),
            OsStr::new(&gui()),
            self.plist.as_os_str(),
        ])
    }

    fn remove(&self) -> io::Result<()> {
        self.unload();
        match std::fs::remove_file(&self.plist) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn service(&self) -> String {
        format!("{}/{}", gui(), self.label)
    }

    fn is_loaded(&self) -> bool {
        launchctl(&[OsStr::new("print"), OsStr::new(&self.service())]).is_ok()
    }

    fn unload(&self) {
        // One bootout doesn't always unload a job that has just been loaded.
        for _ in 0..30 {
            _ = launchctl(&[OsStr::new("bootout"), OsStr::new(&self.service())]);
            if !self.is_loaded() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn var(env: &NSDictionary<NSString, AnyObject>, key: &NSString) -> Option<String> {
    let value = env.objectForKey(key)?.downcast::<NSString>().ok()?;
    Some(value.to_string()).filter(|value| !value.is_empty())
}

fn viewer(domain: Option<&str>) -> Option<String> {
    let value = in_domain(domain, |domain| {
        CFPreferencesCopyValue(
            &CFString::from_static_str(KEY),
            domain,
            unsafe { kCFPreferencesCurrentUser },
            unsafe { kCFPreferencesAnyHost },
        )
    })?;
    Some(value.downcast::<CFString>().ok()?.to_string())
}

fn set_viewer(domain: Option<&str>, id: Option<&str>) -> io::Result<()> {
    let key = CFString::from_static_str(KEY);
    let value = id.map(CFString::from_str);
    let (user, host) = unsafe { (kCFPreferencesCurrentUser, kCFPreferencesAnyHost) };
    let saved = in_domain(domain, |domain| {
        unsafe {
            CFPreferencesSetValue(
                &key,
                value.as_deref().map(|value| value.as_ref()),
                domain,
                user,
                host,
            );
        }
        CFPreferencesSynchronize(domain, user, host)
    });
    if saved {
        Ok(())
    } else {
        Err(io::Error::other("the preferences could not be saved"))
    }
}

fn in_domain<R>(domain: Option<&str>, run: impl FnOnce(&CFString) -> R) -> R {
    match domain {
        Some(domain) => run(&CFString::from_str(domain)),
        None => run(unsafe { kCFPreferencesAnyApplication }),
    }
}

fn is_installed(id: &str) -> bool {
    app_paths(id)
        .iter()
        .any(|path| !is_trashed(path) && path.exists())
}

#[allow(deprecated)]
fn app_paths(id: &str) -> Vec<PathBuf> {
    let urls = unsafe {
        LSCopyApplicationURLsForBundleIdentifier(&CFString::from_str(id), std::ptr::null_mut())
    };
    let Some(urls) = urls else {
        return Vec::new();
    };
    // SAFETY: Launch Services answers with an array of URLs.
    let urls = unsafe { urls.cast_unchecked::<CFURL>() };
    urls.iter()
        .filter_map(|url| url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle))
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_trashed(path: &Path) -> bool {
    path.components()
        .any(|part| matches!(part.as_os_str().to_str(), Some(".Trash" | ".Trashes")))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn gui() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchctl(args: &[&OsStr]) -> io::Result<()> {
    let out = Command::new("/bin/launchctl").args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Instant, SystemTime};

    use super::*;

    const FINDER: &str = "com.apple.finder";
    const OTHER: &str = "com.example.another-viewer";
    const NEVER_INSTALLED: &str = "dev.cafekery.nativegpui.viewer-test.never-installed";

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Scratch {
        dir: PathBuf,
        site: Site,
        id: String,
        bundle: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Option<Scratch> {
            if launchctl(&[OsStr::new("print"), OsStr::new(&gui())]).is_err() {
                eprintln!("skipping: no launchd GUI domain here");
                return None;
            }
            let tag = format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let dir = std::env::temp_dir().join(format!("finder-fs-viewer-{name}-{tag}"));
            _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let id = format!("dev.cafekery.nativegpui.viewer-test.{name}.{tag}");
            let bundle = dir.join("Q&A <Viewer> 'Test'.app");
            make_bundle(&bundle, &id);
            let label = format!("{id}.viewer-guard");
            let site = Site {
                domain: Some(dir.join("prefs").to_str().unwrap().to_owned()),
                plist: dir.join(format!("{label}.plist")),
                label,
            };
            Some(Scratch {
                dir,
                site,
                id,
                bundle,
            })
        }

        fn viewer(&self) -> Option<String> {
            viewer(self.site.domain.as_deref())
        }

        fn set_viewer(&self, id: Option<&str>) {
            set_viewer(self.site.domain.as_deref(), id).unwrap();
        }

        fn runs(&self) -> Option<usize> {
            let out = Command::new("/bin/launchctl")
                .arg("print")
                .arg(self.site.service())
                .output()
                .unwrap();
            let printed = String::from_utf8_lossy(&out.stdout);
            if !printed
                .lines()
                .any(|line| line.trim() == "state = not running")
            {
                return None;
            }
            printed
                .lines()
                .find_map(|line| line.trim().strip_prefix("runs = ")?.parse().ok())
        }

        /// launchd adds a run for the watch path to the one at load; this waits out both and the throttle after them.
        fn idle_runs(&self) -> usize {
            thread::sleep(Duration::from_secs(3));
            let mut runs = None;
            until("the guard to idle", || {
                runs = self.runs();
                runs.is_some()
            });
            thread::sleep(Duration::from_secs(2));
            runs.unwrap()
        }

        fn until_gone(&self) {
            until("the guard to remove itself", || !self.site.is_loaded());
            assert!(!self.site.plist.exists());
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            self.site.unload();
            _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn make_bundle(bundle: &Path, id: &str) {
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        let info = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>{id}</string></dict></plist>\n"
        );
        std::fs::write(bundle.join("Contents/Info.plist"), info).unwrap();
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "timed out waiting for {what}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn a_deleted_bundle_hands_the_viewer_back() {
        let Some(scratch) = Scratch::new("deleted") else {
            return;
        };
        scratch.set_viewer(Some(FINDER));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        assert_eq!(scratch.viewer(), Some(scratch.id.clone()));
        let guard = scratch.site.guard().unwrap();
        assert_eq!(guard.bundle, scratch.bundle);
        assert_eq!(guard.previous.as_deref(), Some(FINDER));
        assert!(guard.is_loaded);

        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        scratch.until_gone();
        assert_eq!(scratch.viewer().as_deref(), Some(FINDER));
    }

    #[test]
    fn a_previous_viewer_since_deleted_leaves_finder() {
        let Some(scratch) = Scratch::new("previous-gone") else {
            return;
        };
        scratch.set_viewer(Some(NEVER_INSTALLED));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        scratch.until_gone();
        assert_eq!(scratch.viewer(), None);
    }

    #[test]
    fn the_guard_leaves_a_viewer_picked_later_alone() {
        let Some(scratch) = Scratch::new("picked-later") else {
            return;
        };
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        scratch.set_viewer(Some(OTHER));
        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        scratch.until_gone();
        assert_eq!(scratch.viewer().as_deref(), Some(OTHER));
    }

    #[test]
    fn a_bundle_replaced_in_place_keeps_the_viewer_and_stays_watched() {
        let Some(scratch) = Scratch::new("replaced") else {
            return;
        };
        scratch.set_viewer(Some(FINDER));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        let runs = scratch.idle_runs();

        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        thread::sleep(Duration::from_millis(500));
        make_bundle(&scratch.bundle, &scratch.id);
        until("the guard to look again", || scratch.runs() > Some(runs));
        assert_eq!(scratch.viewer(), Some(scratch.id.clone()));
        assert!(scratch.site.is_loaded() && scratch.site.plist.exists());

        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        scratch.until_gone();
        assert_eq!(scratch.viewer().as_deref(), Some(FINDER));
    }

    #[test]
    fn an_app_launch_services_still_finds_keeps_the_viewer() {
        let Some(scratch) = Scratch::new("still-found") else {
            return;
        };
        scratch.site.on(FINDER, &scratch.bundle).unwrap();
        let runs = scratch.idle_runs();
        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        until("the guard to look", || scratch.runs() > Some(runs));
        assert_eq!(scratch.viewer().as_deref(), Some(FINDER));
        assert!(scratch.site.is_loaded() && scratch.site.plist.exists());
    }

    #[test]
    fn off_restores_the_previous_viewer_and_drops_the_guard() {
        let Some(scratch) = Scratch::new("off") else {
            return;
        };
        scratch.set_viewer(Some(FINDER));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        scratch.site.off(&scratch.id).unwrap();
        assert_eq!(scratch.viewer().as_deref(), Some(FINDER));
        assert!(!scratch.site.is_loaded() && !scratch.site.plist.exists());
    }

    #[test]
    fn off_forgets_a_previous_viewer_since_deleted() {
        let Some(scratch) = Scratch::new("off-gone") else {
            return;
        };
        scratch.set_viewer(Some(NEVER_INSTALLED));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        scratch.site.off(&scratch.id).unwrap();
        assert_eq!(scratch.viewer(), None);
    }

    #[test]
    fn off_leaves_a_viewer_picked_later_alone() {
        let Some(scratch) = Scratch::new("off-other") else {
            return;
        };
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        scratch.set_viewer(Some(OTHER));
        scratch.site.off(&scratch.id).unwrap();
        assert_eq!(scratch.viewer().as_deref(), Some(OTHER));
        assert!(!scratch.site.is_loaded() && !scratch.site.plist.exists());
    }

    #[test]
    fn turning_on_twice_keeps_the_first_previous_viewer() {
        let Some(scratch) = Scratch::new("twice") else {
            return;
        };
        scratch.set_viewer(Some(FINDER));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        assert_eq!(scratch.site.previous().as_deref(), Some(FINDER));
    }

    #[test]
    fn follow_points_the_guard_at_the_bundle_that_launched() {
        let Some(scratch) = Scratch::new("follow") else {
            return;
        };
        scratch.set_viewer(Some(FINDER));
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        let written = || {
            std::fs::metadata(&scratch.site.plist)
                .unwrap()
                .modified()
                .unwrap()
        };
        let first: SystemTime = written();
        scratch.site.follow(&scratch.id, &scratch.bundle).unwrap();
        assert_eq!(written(), first);

        let moved = scratch.dir.join("Moved.app");
        make_bundle(&moved, &scratch.id);
        scratch.site.follow(&scratch.id, &moved).unwrap();
        let guard = scratch.site.guard().unwrap();
        assert_eq!(guard.bundle, moved);
        assert_eq!(guard.previous.as_deref(), Some(FINDER));
        assert!(guard.is_loaded);

        std::fs::remove_dir_all(&moved).unwrap();
        scratch.until_gone();
        assert_eq!(scratch.viewer().as_deref(), Some(FINDER));
    }

    #[test]
    fn follow_guards_a_viewer_set_before_there_was_a_guard() {
        let Some(scratch) = Scratch::new("unguarded") else {
            return;
        };
        scratch.set_viewer(Some(scratch.id.as_str()));
        scratch.site.follow(&scratch.id, &scratch.bundle).unwrap();
        let guard = scratch.site.guard().unwrap();
        assert_eq!(guard.previous, None);
        assert!(guard.is_loaded);

        std::fs::remove_dir_all(&scratch.bundle).unwrap();
        scratch.until_gone();
        assert_eq!(scratch.viewer(), None);
    }

    #[test]
    fn follow_drops_a_guard_another_viewer_outlived() {
        let Some(scratch) = Scratch::new("outlived") else {
            return;
        };
        scratch.site.on(&scratch.id, &scratch.bundle).unwrap();
        scratch.set_viewer(Some(OTHER));
        scratch.site.follow(&scratch.id, &scratch.bundle).unwrap();
        assert!(!scratch.site.is_loaded() && !scratch.site.plist.exists());
        assert_eq!(scratch.viewer().as_deref(), Some(OTHER));
    }

    struct Apps(PathBuf);

    impl Drop for Apps {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Launch Services skips temporary folders and bundles without an executable.
    fn make_app(dir: &Path, name: &str, id: &str) -> PathBuf {
        let bundle = dir.join(name);
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let info = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>{id}</string>\
             <key>CFBundlePackageType</key><string>APPL</string>\
             <key>CFBundleExecutable</key><string>app</string></dict></plist>\n"
        );
        std::fs::write(bundle.join("Contents/Info.plist"), info).unwrap();
        std::fs::copy("/usr/bin/true", bundle.join("Contents/MacOS/app")).unwrap();
        bundle
    }

    #[test]
    fn unregister_others_forgets_every_copy_but_our_own() {
        let Some(home) = crate::home() else {
            return;
        };
        let tag = format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let apps = Apps(home.join(format!("Library/Caches/finder-fs-viewer-stale-{tag}")));
        let id = format!("dev.cafekery.nativegpui.viewer-test.stale.{tag}");
        let (own, stale) = (
            make_app(&apps.0, "Own.app", &id),
            make_app(&apps.0, "Stale.app", &id),
        );
        register(&own);
        register(&stale);
        let known = |path: &Path| {
            app_paths(&id)
                .iter()
                .any(|at| canonical(at) == canonical(path))
        };
        assert!(known(&own) && known(&stale));

        unregister_others(&[&id], &own);
        assert!(known(&own) && !known(&stale));
        unregister_others(&[&id], &apps.0);
        assert!(!known(&own));
    }

    #[test]
    fn a_translocated_bundle_is_never_guarded() {
        let Some(scratch) = Scratch::new("translocated") else {
            return;
        };
        let translocated =
            Path::new("/private/var/folders/x/T/AppTranslocation/1234/d/Viewer Test.app");
        assert!(is_translocated(translocated) && !is_translocated(&scratch.bundle));
        let refused = scratch.site.on(&scratch.id, translocated).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::Unsupported);
        assert_eq!(scratch.viewer(), None);
        assert!(!scratch.site.plist.exists());
    }
}
