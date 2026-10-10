# Found'er

Found'er is a recreation of the macOS Finder. It is built on native-gpui, which puts real AppKit controls inside a GPUI window, so its sidebar, toolbar, menus and materials are the ones macOS draws.

Found'er is free and open source under the MIT license.

## Requirements

* macOS 26 or later.
* A Mac with Apple silicon. The app is built for arm64 only.

## Install

### Build from source

You need Rust (rustc 1.98 or later), Xcode for the app icon, and cargo-about for the third-party notices.

```sh
git clone https://github.com/Cafekery/NativeGPUI.git
cd NativeGPUI
cargo install cargo-about --locked --features cli
./scripts/bundle.sh finder_demo
```

This builds `dist/Found'er.app`. Drag it into your Applications folder, then open it from there.

`bundle.sh` signs the app with your Developer ID or Apple Development certificate if you have one. If not, it creates a self-signed "NativeGPUI Dev" certificate in `~/Library/Keychains/nativegpui-dev.keychain-db` and signs with that, so the permissions you grant survive a rebuild.

### A copy someone else built

Found'er is not notarized by Apple, so macOS will not open a downloaded copy the first time.

1. Move Found'er.app to your Applications folder before you open it. Opened straight from Downloads, macOS runs it from a hidden read-only location, and the default file browser item stays disabled.
2. Open it. macOS says Apple could not verify it. Click Done.
3. Open System Settings → Privacy & Security. Under Security, click Open Anyway next to the message about Found'er, and confirm.

Only do this for a copy from someone you trust.

## First launch

Found'er has no permission screen of its own. macOS asks on its behalf when Found'er first touches something protected:

* Opening your Desktop, Documents or Downloads folder.
* Opening an external disk or a network share.
* Opening iCloud Drive or another cloud folder.
* Opening other apps' data folders while you browse a Library folder.
* Drawing the icons of the apps in /Applications, which can raise an App Management prompt.

Allow the ones you want Found'er to show. Found'er works without the rest.

### The search index

On launch Found'er indexes the names of the files on your startup disk, so searches and folder sizes come back at once. The index lives in `~/Library/Application Support/FinderDemo/fsearch` and takes several hundred MB: about 700–800 MB on a full disk. The first launch builds it in the background, and search answers once it's done. Later launches load it and catch up on what changed while Found'er was closed.

The index skips the folders that would raise a privacy prompt: Desktop, Documents, Downloads, iCloud Drive and other cloud folders, other apps' containers, the Photos library, and other disks. Search results also leave out anything inside your Desktop, Documents, Downloads, Pictures, Music, Movies and Library folders.

### Full Disk Access

Full Disk Access is optional, and Found'er never asks for it. With it, macOS stops asking folder by folder, and folders macOS otherwise keeps from apps (such as `~/Library/Mail`) are indexed on the next launch, so folder sizes include them. It does not widen what search shows.

To grant it, open System Settings → Privacy & Security → Full Disk Access, turn on Found'er (or click + and choose Found'er.app), then quit and reopen Found'er.

## Default file browser

Found'er menu → Use Found'er as Default File Browser sets the system's `NSFileViewer` default to Found'er's bundle id, `dev.cafekery.nativegpui.finder_demo`. Other apps' Show in Finder then opens Found'er. macOS keeps Finder as the app that opens folders; `NSFileViewer` is the only setting another app can take over.

Turning it on also installs a small guard, `~/Library/LaunchAgents/dev.cafekery.nativegpui.finder_demo.viewer-guard.plist`. macOS shows a "Background Items Added" notice and lists the guard as "sh", the shell it runs in, under System Settings → General → Login Items & Extensions; leave it allowed. The guard runs at login and once an hour, and within a few seconds of Found'er.app being deleted or moved to the Trash. If Found'er is gone, it puts back the file viewer you had before (or Finder, if that app is gone too) and removes itself. It never touches `NSFileViewer` if it names another app. If you move Found'er, it points the guard at the new place on its next launch.

The item is disabled while Found'er runs from a hidden read-only location (opened from Downloads without being moved). Move it to Applications first.

To undo it, uncheck the item. That restores the file viewer you had before and removes the guard.

To undo it by hand, first check that the setting is Found'er's:

```sh
defaults read -g NSFileViewer
```

Only if that prints `dev.cafekery.nativegpui.finder_demo`, run:

```sh
defaults delete -g NSFileViewer
launchctl bootout gui/$(id -u)/dev.cafekery.nativegpui.finder_demo.viewer-guard
rm ~/Library/LaunchAgents/dev.cafekery.nativegpui.finder_demo.viewer-guard.plist
```

## Uninstall

1. If Found'er is your default file browser, uncheck Found'er menu → Use Found'er as Default File Browser.
2. Quit Found'er from the Found'er menu. Closing its windows leaves it running.
3. Move `/Applications/Found'er.app` to the Trash.
4. Remove what it left behind:

```sh
rm -rf ~/Library/Application\ Support/FinderDemo
defaults delete dev.cafekery.nativegpui.finder_demo
tccutil reset All dev.cafekery.nativegpui.finder_demo
```

These are every file and setting Found'er keeps:

| what | where |
|---|---|
| search index, file kinds, network servers it has seen, sidebar items | `~/Library/Application Support/FinderDemo` |
| settings, view options, windows kept across quits | `~/Library/Preferences/dev.cafekery.nativegpui.finder_demo.plist` (remove it with `defaults delete`, not `rm`, so macOS doesn't write it back) |
| the default file browser guard, if you turned that on | `~/Library/LaunchAgents/dev.cafekery.nativegpui.finder_demo.viewer-guard.plist` |
| Full Disk Access and the folder permissions you granted | System Settings → Privacy & Security (`tccutil reset` clears them) |
| saved searches, only the ones you saved | `~/Library/Saved Searches/*.savedSearch`. Finder reads these too; delete only the ones you don't want |

Found'er turns off AppKit's saved window state and keeps nothing in `~/Library/Caches`. Its temporary files go to the per-user temporary folder, which macOS clears.

If you built it from source, also remove the checkout (with its `target/` and `dist/` folders). If `bundle.sh` made its own signing certificate, delete that keychain with `security delete-keychain ~/Library/Keychains/nativegpui-dev.keychain-db`. Running it with `cargo run` instead of the app stores settings under `finder_demo`; remove them with `defaults delete finder_demo`.

## Known limitations

Found'er does not do everything Finder does yet. [docs/finder-gaps.md](docs/finder-gaps.md) lists the gaps.

## Acknowledgements and license

Help → Acknowledgements lists the open-source software Found'er is built with, and their licenses.

Found'er and native-gpui are released under the MIT license; see [LICENSE](LICENSE). Its search engine, fsearch, is by Noah Dunnagan and is also under the MIT license; see [crates/fsearch/LICENSE](crates/fsearch/LICENSE).

Found'er is not affiliated with or endorsed by Apple. Finder and macOS are trademarks of Apple Inc.

## For developers

The native-gpui crate, the examples and the scripts are described in [docs/native-gpui.md](docs/native-gpui.md). To build, test and send changes, read [CONTRIBUTING.md](CONTRIBUTING.md). To report a security problem, read [SECURITY.md](SECURITY.md).
