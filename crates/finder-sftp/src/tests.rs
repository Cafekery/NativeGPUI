use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use crate::askpass::{Prompt, classify};
use crate::{Address, Error, Kind, Lane, Server};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("finder-sftp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn serve(dir: &Path) -> Server {
    let dir = dir.to_owned();
    Server::connect_command(move || {
        let mut command = Command::new("/usr/libexec/sftp-server");
        command.current_dir(&dir);
        command
    })
    .unwrap()
}

fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn home_is_the_server_working_directory() {
    let dir = scratch("home");
    let server = serve(&dir);
    assert_eq!(server.home(), path(&dir));
    assert!(
        server
            .extensions()
            .iter()
            .any(|name| name == "posix-rename@openssh.com")
    );
    assert_eq!(
        server.realpath(&format!("{}/./.", path(&dir))).unwrap(),
        path(&dir)
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lists_entries_with_attributes_and_without_dots() {
    let dir = scratch("list");
    fs::write(dir.join("five"), b"12345").unwrap();
    fs::create_dir(dir.join("folder")).unwrap();
    symlink("five", dir.join("link")).unwrap();
    for index in 0..250 {
        fs::write(dir.join(format!("many-{index}")), b"").unwrap();
    }
    let server = serve(&dir);

    let entries = server.list(path(&dir)).unwrap();
    assert_eq!(entries.len(), 253);
    assert!(
        entries
            .iter()
            .all(|entry| entry.name != "." && entry.name != "..")
    );
    let find = |name: &str| entries.iter().find(|entry| entry.name == name).unwrap();
    assert_eq!(find("five").attrs.size, Some(5));
    assert_eq!(find("five").attrs.kind(), Kind::File);
    assert_eq!(find("folder").attrs.kind(), Kind::Dir);
    assert_eq!(find("link").attrs.kind(), Kind::Symlink);
    assert!(find("five").longname.contains("five"));

    let link = format!("{}/link", path(&dir));
    assert_eq!(server.stat(&link).unwrap().kind(), Kind::Symlink);
    assert_eq!(server.stat_follow(&link).unwrap().kind(), Kind::File);
    assert_eq!(server.readlink(&link).unwrap(), "five");
    let made = format!("{}/made", path(&dir));
    server.symlink("folder", &made).unwrap();
    assert_eq!(fs::read_link(&made).unwrap(), Path::new("folder"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn makes_renames_and_removes() {
    let dir = scratch("ops");
    let server = serve(&dir);
    let at = |name: &str| format!("{}/{name}", path(&dir));

    server.mkdir(&at("a")).unwrap();
    assert!(server.mkdir(&at("a")).is_err());
    server.rename(&at("a"), &at("b")).unwrap();
    assert!(dir.join("b").is_dir() && !dir.join("a").exists());
    server.rmdir(&at("b")).unwrap();
    assert!(!dir.join("b").exists());

    fs::write(dir.join("file"), b"x").unwrap();
    server.set_permissions(&at("file"), 0o600).unwrap();
    assert_eq!(
        fs::metadata(dir.join("file")).unwrap().permissions().mode() & 0o777,
        0o600
    );
    server.set_mtime(&at("file"), 1_000_000_000).unwrap();
    assert_eq!(server.stat(&at("file")).unwrap().mtime, Some(1_000_000_000));
    server.remove(&at("file")).unwrap();
    assert!(!dir.join("file").exists());

    let space = server.statvfs(path(&dir)).unwrap();
    assert!(space.total > 0 && space.available <= space.total);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_paths_are_not_found() {
    let dir = scratch("missing");
    let server = serve(&dir);
    let missing = format!("{}/missing", path(&dir));
    assert!(server.stat(&missing).unwrap_err().is_not_found());
    assert!(server.list(&missing).unwrap_err().is_not_found());
    assert!(server.remove(&missing).unwrap_err().is_not_found());
    let local = dir.join("never");
    let error = server
        .download(&missing, &local, &mut |_, _| true)
        .unwrap_err();
    assert!(error.is_not_found());
    assert!(!local.exists());
    fs::remove_dir_all(&dir).unwrap();
}

fn round_trip(len: usize) -> Server {
    let dir = scratch(&format!("trip-{len}"));
    let server = serve(&dir);
    let bytes = noise(len);
    fs::write(dir.join("local"), &bytes).unwrap();
    let remote = format!("{}/remote", path(&dir));

    let mut last = (0, 0);
    server
        .upload(&dir.join("local"), &remote, &mut |done, total| {
            last = (done, total);
            true
        })
        .unwrap();
    assert_eq!(last, (len as u64, len as u64));
    assert!(fs::read(dir.join("remote")).unwrap() == bytes);

    server
        .download(&remote, &dir.join("back"), &mut |_, _| true)
        .unwrap();
    assert!(fs::read(dir.join("back")).unwrap() == bytes);
    fs::remove_dir_all(&dir).unwrap();
    server
}

#[test]
fn round_trips_a_file_split_across_sessions() {
    let server = round_trip(20 << 20);
    assert!(server.inner.pool.lock().sessions[Lane::Bulk as usize].is_some());
}

#[test]
fn round_trips_a_small_file() {
    let server = round_trip(3 << 10);
    assert!(server.inner.pool.lock().sessions[Lane::Bulk as usize].is_none());
}

#[test]
fn round_trips_an_empty_file() {
    round_trip(0);
}

#[test]
fn transfers_keep_the_modification_time() {
    let dir = scratch("mtime");
    let server = serve(&dir);
    let local = dir.join("local");
    fs::write(&local, b"dated").unwrap();
    fs::set_permissions(&local, fs::Permissions::from_mode(0o750)).unwrap();
    let then = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(&local)
        .unwrap()
        .set_modified(then)
        .unwrap();

    let remote = format!("{}/remote", path(&dir));
    server.upload(&local, &remote, &mut |_, _| true).unwrap();
    let attrs = server.stat(&remote).unwrap();
    assert_eq!(attrs.mtime, Some(1_000_000_000));
    assert_eq!(attrs.permissions.map(|mode| mode & 0o777), Some(0o750));

    server
        .download(&remote, &dir.join("back"), &mut |_, _| true)
        .unwrap();
    let modified = fs::metadata(dir.join("back")).unwrap().modified().unwrap();
    assert_eq!(modified, then);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cancelling_leaves_no_partial_file() {
    let dir = scratch("cancel");
    let server = serve(&dir);
    fs::write(dir.join("source"), noise(20 << 20)).unwrap();
    let source = format!("{}/source", path(&dir));

    let local = dir.join("download");
    let error = server
        .download(&source, &local, &mut |_, _| false)
        .unwrap_err();
    assert!(matches!(error, Error::Cancelled));
    assert!(!local.exists());

    let remote = format!("{}/upload", path(&dir));
    let error = server
        .upload(&dir.join("source"), &remote, &mut |_, _| false)
        .unwrap_err();
    assert!(matches!(error, Error::Cancelled));
    assert!(server.stat(&remote).unwrap_err().is_not_found());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_parked_progress_callback_pauses_the_transfer() {
    let dir = scratch("pause");
    let server = serve(&dir);
    let bytes = noise(1 << 20).repeat(256);
    fs::write(dir.join("source"), &bytes).unwrap();
    let remote = dir.join("remote");
    let written = || fs::metadata(&remote).map_or(0, |meta| meta.len());
    let mut parked = None;
    server
        .upload(&dir.join("source"), path(&remote), &mut |_, _| {
            if parked.is_none() {
                std::thread::sleep(Duration::from_millis(150));
                let early = written();
                std::thread::sleep(Duration::from_millis(150));
                parked = Some((early, written()));
            }
            true
        })
        .unwrap();
    let (early, late) = parked.unwrap();
    assert!(late < bytes.len() as u64, "the upload ran to the end");
    assert_eq!(early, late, "bytes kept landing while parked");
    assert!(fs::read(&remote).unwrap() == bytes);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lists_while_a_download_waits() {
    let dir = scratch("busy");
    let server = serve(&dir);
    let fifo = dir.join("fifo");
    assert!(
        Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let download = std::thread::spawn({
        let (server, remote, local) = (server.clone(), path(&fifo).to_owned(), dir.join("back"));
        move || server.download(&remote, &local, &mut |_, _| true)
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        server
            .list(path(&dir))
            .unwrap()
            .iter()
            .any(|entry| entry.name == "fifo")
    );
    fs::write(&fifo, b"late").unwrap();
    let _ = download.join().unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reads_small_files_whole() {
    let dir = scratch("read");
    let server = serve(&dir);
    let bytes = noise(600 << 10);
    fs::write(dir.join("file"), &bytes).unwrap();
    let file = format!("{}/file", path(&dir));
    assert!(server.read_all(&file, 1 << 20).unwrap() == bytes);
    assert!(matches!(
        server.read_all(&file, 1000),
        Err(Error::Protocol(_))
    ));
    assert!(server.read_range(&file, 1000, 50).unwrap() == bytes[1000..1050]);
    assert!(server.read_range(&file, (600 << 10) - 10, 100).unwrap() == bytes[(600 << 10) - 10..]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn copies_on_the_server() {
    let dir = scratch("copy");
    let server = serve(&dir);
    fs::write(dir.join("from"), b"copied").unwrap();
    let at = |name: &str| format!("{}/{name}", path(&dir));
    server.copy_data(&at("from"), &at("to")).unwrap().unwrap();
    assert_eq!(fs::read(dir.join("to")).unwrap(), b"copied");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_disconnected_server_refuses_calls() {
    let dir = scratch("disconnect");
    let server = serve(&dir);
    let clone = server.clone();
    server.disconnect();
    assert!(matches!(clone.stat(path(&dir)), Err(Error::Disconnected)));
    assert!(matches!(server.list(path(&dir)), Err(Error::Disconnected)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dead_session_is_replaced() {
    let dir = scratch("respawn");
    let server = serve(&dir);
    server.inner.session(0).unwrap().kill();
    assert_eq!(server.realpath(path(&dir)).unwrap(), path(&dir));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lanes_ride_their_own_sessions() {
    let dir = scratch("lanes");
    fs::write(dir.join("file"), b"lanes").unwrap();
    let server = serve(&dir);
    let file = format!("{}/file", path(&dir));
    assert_eq!(
        server.on(Lane::Bulk).read_range(&file, 0, 5).unwrap(),
        b"lanes"
    );
    assert_eq!(
        server.on(Lane::Preview).read_range(&file, 1, 2).unwrap(),
        b"an"
    );
    let slot = |lane: Lane| server.inner.pool.lock().sessions[lane as usize].clone();
    let (bulk, preview) = (slot(Lane::Bulk).unwrap(), slot(Lane::Preview).unwrap());
    assert!(!std::sync::Arc::ptr_eq(&bulk, &preview));
    assert!(slot(Lane::Interactive).is_none());

    server.inner.session(Lane::Control as usize).unwrap().kill();
    let listing = server.on(Lane::Listing);
    assert_eq!(listing.list(path(&dir)).unwrap().len(), 1);
    assert_eq!(listing.stat(&file).unwrap().size, Some(5));
    assert!(!slot(Lane::Control).unwrap().alive());
    assert!(slot(Lane::Listing).unwrap().alive());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_login_carries_what_ssh_said() {
    let error = Server::connect_command(|| {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo 'Permission denied (publickey).' >&2; exit 255"]);
        command
    })
    .err()
    .unwrap();
    assert!(matches!(&error, Error::Connect(text) if text == "Permission denied (publickey)."));
}

#[test]
fn parses_addresses() {
    let address = Address::parse("sftp://jace@100.81.202.94/media/Storage/Jace").unwrap();
    assert_eq!(address.user.as_deref(), Some("jace"));
    assert_eq!(address.host, "100.81.202.94");
    assert_eq!(address.port, None);
    assert_eq!(address.path, "/media/Storage/Jace");
    assert_eq!(address.root(), "sftp://jace@100.81.202.94");

    let address = Address::parse("sftp://host:2222").unwrap();
    assert_eq!((address.host.as_str(), address.port), ("host", Some(2222)));
    assert_eq!((address.user.as_deref(), address.path.as_str()), (None, ""));
    assert_eq!(address.root(), "sftp://host:2222");

    let address = Address::parse("user@host").unwrap();
    assert_eq!(address.user.as_deref(), Some("user"));
    assert_eq!(address.root(), "sftp://user@host");
    assert_eq!(Address::parse("host").unwrap().root(), "sftp://host");
    assert_eq!(Address::parse("SFTP://host/").unwrap().path, "/");

    let address = Address::parse("sftp://me@[fd7a::1]:22/x").unwrap();
    assert_eq!((address.host.as_str(), address.port), ("fd7a::1", Some(22)));
    assert_eq!(address.root(), "sftp://me@[fd7a::1]:22");

    for bad in [
        "",
        "sftp://",
        "smb://host/share",
        "@host",
        "user@",
        "host:",
        "host:port",
        "host:0",
        "two words",
        "-oProxyCommand=x",
        "sftp://-ox@host",
        "[::1",
    ] {
        assert_eq!(Address::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn classifies_prompts() {
    assert_eq!(classify("jace@host's password: "), Prompt::Password);
    assert_eq!(classify("(jace@host) Password: "), Prompt::Password);
    assert_eq!(classify("Password for jace@host"), Prompt::Password);
    let host_key = "The authenticity of host 'h' can't be established.\n\
        Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
    assert!(
        matches!(classify(host_key), Prompt::HostKey { text } if text.contains("authenticity"))
    );
    assert!(matches!(
        classify("Enter passphrase for key: "),
        Prompt::Other(_)
    ));
}
