use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::meter::{Left, Meter};
use super::*;
use crate::ops::{Op, name_of};
use crate::purge;

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("finder-fs-transfer-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn sparse(path: &Path, len: u64) {
    File::create(path).unwrap().set_len(len).unwrap();
}

fn root_user() -> bool {
    let root = unsafe { libc::geteuid() } == 0;
    if root {
        eprintln!("skipping: root reads anything whatever its mode says");
    }
    root
}

fn staged_in(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| name_of(path).starts_with(".finder-fs-"))
                .collect()
        })
        .unwrap_or_default()
}

fn request(kind: Kind, items: Vec<PathBuf>, into: &Path) -> Request {
    Request::new(kind, items, into, Conflict::KeepBoth, OnError::Continue)
}

fn run(kind: Kind, items: Vec<PathBuf>, into: &Path) -> Outcome {
    Transfer::default().run(request(kind, items, into))
}

pub(crate) struct RamDisk {
    device: String,
    pub(crate) mount: PathBuf,
}

impl RamDisk {
    pub(crate) fn new(mb: u64, format: &str, name: &str) -> Option<RamDisk> {
        let out = Command::new("hdiutil")
            .args(["attach", "-nomount", &format!("ram://{}", mb * 2048)])
            .output()
            .ok()?;
        let device = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || device.is_empty() {
            eprintln!("skipping: no RAM disk");
            return None;
        }
        let disk = RamDisk {
            device,
            mount: PathBuf::from(format!("/Volumes/{name}")),
        };
        let erased = Command::new("diskutil")
            .args(["eraseVolume", format, name, &disk.device])
            .output()
            .is_ok_and(|out| out.status.success());
        if !erased {
            eprintln!("skipping: could not format a RAM disk as {format}");
            return None;
        }
        Some(disk)
    }
}

impl Drop for RamDisk {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", &self.device, "-force"])
            .output();
    }
}

fn volume_name(tag: &str) -> String {
    format!("XF{}{tag}", std::process::id() % 100_000)
}

fn held_mid_file(transfer: &Transfer, request: Request) -> thread::JoinHandle<Outcome> {
    transfer.pause();
    let running = transfer.clone();
    let handle = thread::spawn(move || running.run(request));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let progress = transfer.progress();
        if progress.current.is_some() && progress.bytes_done > 0 {
            return handle;
        }
        assert!(Instant::now() < deadline, "no file started: {progress:?}");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_copy_lands_every_item_and_counts_it() {
    let dir = temp_dir("lands");
    let (src, dst) = (dir.join("src"), dir.join("dst"));
    let folder = src.join("Folder");
    write(&folder.join("b.txt"), &[7; 5000]);
    write(&folder.join("c").join("d.bin"), &[9; 3 << 20]);
    fs::create_dir_all(folder.join("e")).unwrap();
    std::os::unix::fs::symlink("b.txt", folder.join("link")).unwrap();
    write(&src.join("f.txt"), b"loose");
    fs::create_dir_all(&dst).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();

    let transfer = Transfer::default();
    let outcome = transfer.run(request(
        Kind::Copy,
        vec![folder.clone(), src.join("f.txt")],
        &dst,
    ));
    let landed = dst.join("Folder");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(outcome.end, End::Finished, "{:?}", outcome.failures);
    assert!(outcome.failures.is_empty());
    assert_eq!(
        outcome.done.results,
        vec![landed.clone(), dst.join("f.txt")]
    );
    assert_eq!(fs::read(landed.join("b.txt")).unwrap(), vec![7; 5000]);
    assert_eq!(
        fs::read(landed.join("c").join("d.bin")).unwrap(),
        vec![9; 3 << 20]
    );
    assert!(landed.join("e").is_dir());
    assert_eq!(
        fs::read_link(landed.join("link")).unwrap(),
        Path::new("b.txt")
    );
    assert_eq!(
        fs::metadata(&landed).unwrap().permissions().mode() & 0o777,
        0o555,
        "a read-only folder takes its mode only once it is filled"
    );
    let progress = transfer.progress();
    assert_eq!(progress.phase, Phase::Done);
    assert_eq!((progress.items_done, progress.items_total), (7, 7));
    let bytes = 5000 + (3 << 20) + 5;
    assert_eq!((progress.bytes_done, progress.bytes_total), (bytes, bytes));
    assert!(staged_in(&landed).is_empty() && staged_in(&dst).is_empty());

    fs::set_permissions(&landed, fs::Permissions::from_mode(0o755)).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn copying_starts_before_the_walk_ends() {
    let Some(disk) = RamDisk::new(160, "HFS+", &volume_name("W")) else {
        return;
    };
    let dir = temp_dir("streams");
    let big = dir.join("big.bin");
    sparse(&big, 64 << 20);
    for i in 0..20 {
        write(&dir.join("many").join(format!("{i}.txt")), b"small");
    }

    let transfer = Transfer::default();
    let handle = held_mid_file(
        &transfer,
        request(Kind::Copy, vec![big, dir.join("many")], &disk.mount),
    );
    let early = transfer.progress();
    transfer.resume();
    let outcome = handle.join().unwrap();
    let late = transfer.progress();

    assert!(
        early.counting,
        "the walk had ended before any copying: {early:?}"
    );
    assert!(early.bytes_done > 0 && early.items_total < late.items_total);
    assert_eq!(
        late.items_total, 22,
        "the big file, the folder and 20 files"
    );
    assert_eq!(outcome.end, End::Finished, "{:?}", outcome.failures);
    assert_eq!(
        fs::metadata(disk.mount.join("big.bin")).unwrap().len(),
        64 << 20
    );
    assert_eq!(fs::read_dir(disk.mount.join("many")).unwrap().count(), 20);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_cancel_leaves_no_part_written_file_and_the_source_whole() {
    let Some(disk) = RamDisk::new(160, "HFS+", &volume_name("C")) else {
        return;
    };
    let dir = temp_dir("cancel");
    let big = dir.join("big.bin");
    sparse(&big, 64 << 20);

    for kind in [Kind::Copy, Kind::Move] {
        let transfer = Transfer::default();
        let handle = held_mid_file(&transfer, request(kind, vec![big.clone()], &disk.mount));
        let caught = transfer.progress().bytes_done;
        transfer.cancel();
        let outcome = handle.join().unwrap();

        assert!(
            caught > 0 && caught < 64 << 20,
            "{kind:?}: not caught halfway"
        );
        assert_eq!(outcome.end, End::Cancelled, "{kind:?}");
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert!(outcome.done.results.is_empty());
        assert!(
            !taken(&disk.mount.join("big.bin")),
            "{kind:?}: a part file has the name"
        );
        assert!(
            staged_in(&disk.mount).is_empty(),
            "{kind:?}: the part file stayed"
        );
        assert_eq!(
            fs::metadata(&big).unwrap().len(),
            64 << 20,
            "{kind:?}: source"
        );
        let progress = transfer.progress();
        assert!(progress.bytes_done <= progress.bytes_total, "{progress:?}");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_move_removes_each_source_only_once_its_copy_is_whole() {
    let Some(disk) = RamDisk::new(160, "HFS+", &volume_name("M")) else {
        return;
    };
    let dir = temp_dir("move");
    let folder = dir.join("Folder");
    write(&folder.join("a.txt"), b"alpha");
    write(&folder.join("inner").join("b.txt"), b"beta");
    let big = dir.join("big.bin");
    sparse(&big, 64 << 20);

    let transfer = Transfer::default();
    let handle = held_mid_file(
        &transfer,
        request(Kind::Move, vec![big.clone(), folder.clone()], &disk.mount),
    );
    File::options()
        .append(true)
        .open(&big)
        .unwrap()
        .write_all(b"written while it was being copied")
        .unwrap();
    transfer.resume();
    let outcome = handle.join().unwrap();

    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    let failure = &outcome.failures[0];
    assert_eq!(failure.source, big);
    assert!(
        failure.error.to_string().contains("changed"),
        "{}",
        failure.error
    );
    assert_eq!(
        fs::metadata(&big).unwrap().len(),
        (64 << 20) + 33,
        "a source that changed mid copy was removed"
    );
    assert!(!taken(&disk.mount.join("big.bin")) && staged_in(&disk.mount).is_empty());

    let moved = disk.mount.join("Folder");
    assert_eq!(fs::read(moved.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        fs::read(moved.join("inner").join("b.txt")).unwrap(),
        b"beta"
    );
    assert!(!folder.exists(), "the moved folder's source stayed");
    assert_eq!(outcome.done.results, vec![moved.clone()]);

    let retry = outcome
        .retry()
        .expect("the changed file can be tried again");
    let again = Transfer::default().run(retry);
    assert!(again.failures.is_empty(), "{:?}", again.failures);
    assert_eq!(
        fs::metadata(disk.mount.join("big.bin")).unwrap().len(),
        (64 << 20) + 33
    );
    assert!(!big.exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_move_keeps_a_locked_source_it_cannot_remove() {
    let Some(disk) = RamDisk::new(40, "HFS+", &volume_name("L")) else {
        return;
    };
    let dir = temp_dir("locked");
    let file = dir.join("locked.txt");
    write(&file, b"locked");
    let lock = |flag: &str| {
        let ok = Command::new("chflags")
            .arg(flag)
            .arg(&file)
            .status()
            .unwrap();
        assert!(ok.success());
    };
    lock("uchg");

    let outcome = run(Kind::Move, vec![file.clone()], &disk.mount);
    let landed = disk.mount.join("locked.txt");
    let still_locked = crate::ops::locked(&landed);
    let _ = Command::new("chflags").arg("nouchg").arg(&landed).status();
    lock("nouchg");

    assert_eq!(
        fs::read(&file).unwrap(),
        b"locked",
        "the locked source went"
    );
    assert_eq!(fs::read(&landed).unwrap(), b"locked");
    assert!(still_locked, "the copy lost its lock");
    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    assert!(outcome.failures[0].error.to_string().contains("locked"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_full_volume_hidden_from_browsing_is_the_one_named() {
    let name = volume_name("H");
    let Some(disk) = RamDisk::new(8, "HFS+", &name) else {
        return;
    };
    let diskutil = |args: &[&str]| {
        Command::new("diskutil")
            .args(args)
            .output()
            .is_ok_and(|out| out.status.success())
    };
    let hidden = diskutil(&["unmount", &disk.device])
        && diskutil(&["mount", "-mountOptions", "nobrowse", &disk.device]);
    if !hidden {
        eprintln!("skipping: could not mount the RAM disk hidden");
        return;
    }
    let dir = temp_dir("hidden");
    let big = dir.join("big.bin");
    sparse(&big, 32 << 20);

    let outcome = run(Kind::Copy, vec![big], &disk.mount);

    let message = outcome.failures.first().map(|f| f.error.to_string());
    let message = message.unwrap_or_default();
    assert!(
        message.contains(&format!("on \u{201c}{name}\u{201d}")),
        "{message}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn failures_are_collected_while_the_rest_lands() {
    if root_user() {
        return;
    }
    let dir = temp_dir("failures");
    let (src, dst) = (dir.join("Folder"), dir.join("dst"));
    write(&src.join("fine.txt"), b"fine");
    write(&src.join("secret.txt"), b"secret");
    write(&src.join("closed").join("inside.txt"), b"inside");
    write(&src.join("after").join("later.txt"), b"later");
    fs::create_dir_all(&dst).unwrap();
    fs::create_dir_all(dir.join("stopped")).unwrap();
    let shut = |mode: u32| {
        fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(mode)).unwrap();
        fs::set_permissions(src.join("closed"), fs::Permissions::from_mode(mode | 0o100)).unwrap();
    };
    shut(0o000);

    let stopped = Transfer::default().run(Request::new(
        Kind::Copy,
        vec![src.clone()],
        &dir.join("stopped"),
        Conflict::KeepBoth,
        OnError::Stop,
    ));
    let outcome = run(Kind::Copy, vec![src.clone()], &dst);
    shut(0o700);

    assert_eq!(stopped.end, End::Stopped);
    assert!(!stopped.failures.is_empty());
    assert_eq!(outcome.end, End::Finished);
    let mut failed: Vec<_> = outcome.failures.iter().map(|f| f.source.clone()).collect();
    failed.sort();
    assert_eq!(failed, vec![src.join("closed"), src.join("secret.txt")]);
    for failure in &outcome.failures {
        let message = failure.error.to_string();
        assert!(
            message.contains("permission") && !message.contains("os error"),
            "{message}"
        );
    }
    let landed = dst.join("Folder");
    assert_eq!(fs::read(landed.join("fine.txt")).unwrap(), b"fine");
    assert_eq!(
        fs::read(landed.join("after").join("later.txt")).unwrap(),
        b"later"
    );
    assert!(!taken(&landed.join("secret.txt")) && staged_in(&landed).is_empty());

    let again = Transfer::default().run(outcome.retry().expect("two items to try again"));
    assert!(again.failures.is_empty(), "{:?}", again.failures);
    assert_eq!(fs::read(landed.join("secret.txt")).unwrap(), b"secret");
    assert_eq!(
        fs::read(landed.join("closed").join("inside.txt")).unwrap(),
        b"inside"
    );
    assert!(
        !taken(&dst.join("Folder 2")),
        "a retry lands where the first try did"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_retried_move_takes_the_folders_it_empties() {
    if root_user() {
        return;
    }
    let Some(disk) = RamDisk::new(40, "HFS+", &volume_name("R")) else {
        return;
    };
    let dir = temp_dir("retried-move");
    let folder = dir.join("Folder");
    let secret = folder.join("inner").join("secret.txt");
    write(&folder.join("fine.txt"), b"fine");
    write(&secret, b"secret");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();

    let outcome = run(Kind::Move, vec![folder.clone()], &disk.mount);
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    assert!(secret.exists(), "the source of what failed went");
    let landed = disk.mount.join("Folder");
    assert_eq!(fs::read(landed.join("fine.txt")).unwrap(), b"fine");

    let again = Transfer::default().run(outcome.retry().expect("the folder to try again"));
    assert!(again.failures.is_empty(), "{:?}", again.failures);
    assert_eq!(
        fs::read(landed.join("inner").join("secret.txt")).unwrap(),
        b"secret"
    );
    assert!(
        !taken(&disk.mount.join("Folder 2")),
        "a retry lands where the first try did"
    );
    assert!(!folder.exists(), "the emptied source folders stayed");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_merge_leaves_files_that_already_match() {
    if !crate::trash_reachable() {
        return;
    }
    let dir = temp_dir("merge");
    let (src, dst) = (dir.join("src").join("Folder"), dir.join("dst"));
    write(&src.join("same.txt"), b"same");
    write(&src.join("changed.txt"), b"new");
    write(&dst.join("Folder").join("same.txt"), b"same");
    write(&dst.join("Folder").join("changed.txt"), b"older");
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for path in [src.join("same.txt"), dst.join("Folder").join("same.txt")] {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }
    let kept = fs::metadata(dst.join("Folder").join("same.txt"))
        .unwrap()
        .ino();

    let outcome = Transfer::default().run(Request::new(
        Kind::Copy,
        vec![src],
        &dst,
        Conflict::Merge,
        OnError::Continue,
    ));

    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let merged = dst.join("Folder");
    assert_eq!(outcome.done.results, vec![merged.clone()]);
    assert_eq!(fs::metadata(merged.join("same.txt")).unwrap().ino(), kept);
    assert_eq!(fs::read(merged.join("changed.txt")).unwrap(), b"new");
    let undo = outcome
        .done
        .undo
        .expect("the replaced file can be put back");
    let Op::Batch(steps) = undo else {
        panic!("a merge undoes in two steps: {undo:?}");
    };
    let Op::PutBack(back) = &steps[1] else {
        panic!("{steps:?}");
    };
    assert_eq!(back.len(), 1, "only the changed file was replaced");
    purge(
        &back
            .iter()
            .map(|restore| restore.item.clone())
            .collect::<Vec<_>>(),
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_volume_without_room_stops_before_it_fills() {
    let Some(disk) = RamDisk::new(24, "HFS+", &volume_name("S")) else {
        return;
    };
    let dir = temp_dir("room");
    let big = dir.join("big.bin");
    sparse(&big, 64 << 20);

    let outcome = run(Kind::Copy, vec![big], &disk.mount);

    assert_eq!(outcome.end, End::Stopped);
    assert!(!outcome.failures.is_empty());
    for failure in &outcome.failures {
        let message = failure.error.to_string();
        assert!(message.contains("enough space"), "{message}");
    }
    assert!(!taken(&disk.mount.join("big.bin")) && staged_in(&disk.mount).is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_total_known_not_to_fit_is_refused_before_anything_moves() {
    let name = volume_name("U");
    let Some(disk) = RamDisk::new(100, "MS-DOS FAT32", &name) else {
        return;
    };
    let ballast = disk.mount.join("ballast.bin");
    File::create(&ballast).unwrap().set_len(70 << 20).unwrap();
    let dir = temp_dir("refused");
    let tree = dir.join("Tree");
    fs::create_dir_all(tree.join("inner")).unwrap();
    for i in 0..40 {
        sparse(&tree.join(format!("{i}.bin")), 1 << 20);
    }
    for i in 0..5 {
        sparse(&tree.join("inner").join(format!("{i}.bin")), 1 << 20);
    }
    let litter = [tree.join(".DS_Store"), tree.join("inner").join(".DS_Store")];
    for path in &litter {
        write(path, b"view settings");
    }

    let outcome = run(Kind::Move, vec![tree.clone()], &disk.mount);

    assert_eq!(outcome.end, End::Stopped);
    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    let message = outcome.failures[0].error.to_string();
    assert!(
        message.starts_with(&format!(
            "There isn't enough space on \u{201c}{name}\u{201d} to move \u{201c}Tree\u{201d}."
        )) && message.contains("You need an additional"),
        "{message}"
    );
    assert!(
        !taken(&disk.mount.join("Tree")) && staged_in(&disk.mount).is_empty(),
        "something was written before the refusal"
    );
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 42, "a source went");
    assert!(
        litter.iter().all(|path| path.exists()),
        "the source lost its view settings"
    );

    fs::remove_file(&ballast).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while crate::volumes::available(&disk.mount) < 60 << 20 {
        assert!(
            Instant::now() < deadline,
            "the volume never gave the room back"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let again = Transfer::default().run(outcome.retry().expect("the refused move to try again"));
    assert_eq!(again.end, End::Finished, "{:?}", again.failures);
    let landed = disk.mount.join("Tree");
    let named = |dir: &Path| {
        let entries = fs::read_dir(dir).unwrap().flatten();
        entries
            .filter(|entry| !entry.file_name().as_encoded_bytes().starts_with(b"._"))
            .count()
    };
    assert_eq!(named(&landed), 41);
    assert_eq!(named(&landed.join("inner")), 5);
    assert!(!tree.exists(), "the moved tree stayed at its source");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn several_items_that_cannot_fit_are_refused_by_the_volume_not_the_first_item() {
    let name = volume_name("V");
    let Some(disk) = RamDisk::new(24, "HFS+", &name) else {
        return;
    };
    let dir = temp_dir("several");
    let items = vec![dir.join("first.bin"), dir.join("second.bin")];
    for item in &items {
        sparse(item, 20 << 20);
    }

    let outcome = run(Kind::Copy, items, &disk.mount);

    assert_eq!(outcome.end, End::Stopped);
    let message = outcome.failures[0].error.to_string();
    assert!(
        message.starts_with(&format!(
            "There isn't enough space on \u{201c}{name}\u{201d} to copy these items."
        )) && !message.contains("first.bin"),
        "{message}"
    );
    assert!(staged_in(&disk.mount).is_empty() && !taken(&disk.mount.join("first.bin")));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_being_written_holds_its_room_without_stopping_the_job() {
    let Some(disk) = RamDisk::new(300, "HFS+", &volume_name("P")) else {
        return;
    };
    let dir = temp_dir("held-room");
    let big = dir.join("big.bin");
    sparse(&big, 200 << 20);

    let transfer = Transfer::default();
    let handle = held_mid_file(&transfer, request(Kind::Copy, vec![big], &disk.mount));
    thread::sleep(SPACE_EVERY + TICK * 2);
    let held = transfer.progress();
    transfer.resume();
    let outcome = handle.join().unwrap();

    assert!(
        held.phase == Phase::Copying && held.failures == 0,
        "a space check counted the file in flight twice: {:?}",
        outcome.failures
    );
    assert_eq!(outcome.end, End::Finished, "{:?}", outcome.failures);
    assert_eq!(
        fs::metadata(disk.mount.join("big.bin")).unwrap().len(),
        200 << 20
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_stop_keeps_the_counts_its_row_showed() {
    if root_user() {
        return;
    }
    let dir = temp_dir("stop-counts");
    let folder = dir.join("Folder");
    for i in 0..30 {
        write(&folder.join(format!("{i}.txt")), &[1; 10_000]);
    }
    let secret = folder.join("secret.txt");
    write(&secret, &[2; 10_000]);
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    let into = dir.join("into");
    fs::create_dir_all(&into).unwrap();

    let transfer = Transfer::default();
    transfer.pause();
    let running = transfer.clone();
    let request = Request::new(
        Kind::Copy,
        vec![folder],
        &into,
        Conflict::KeepBoth,
        OnError::Stop,
    );
    let handle = thread::spawn(move || running.run(request));
    let deadline = Instant::now() + Duration::from_secs(20);
    let planned = loop {
        let progress = transfer.progress();
        if progress.items_total > 0 && !progress.counting {
            break progress;
        }
        assert!(Instant::now() < deadline, "the walk never ended");
        thread::sleep(Duration::from_millis(2));
    };
    transfer.resume();
    let outcome = handle.join().unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let ended = transfer.progress();

    assert_eq!(outcome.end, End::Stopped);
    assert_eq!((planned.items_total, planned.bytes_total), (32, 310_000));
    assert_eq!(
        (ended.items_total, ended.bytes_total),
        (planned.items_total, planned.bytes_total),
        "the totals shrank while the stop wound down"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_fat_volume_gets_neither_oversized_files_nor_finder_litter() {
    let Some(disk) = RamDisk::new(100, "MS-DOS FAT32", &volume_name("F")) else {
        return;
    };
    let dir = temp_dir("fat");
    let folder = dir.join("Folder");
    write(&folder.join("photo.jpg"), b"photo");
    write(&folder.join(".DS_Store"), b"view settings");
    sparse(&folder.join("huge.mov"), (4 << 30) + 1);
    let photo = folder.join("photo.jpg");
    for (name, value) in [
        ("com.apple.metadata:_kMDItemUserTags", "tagged"),
        ("com.apple.metadata:kMDItemWhereFroms", "a download"),
    ] {
        let ok = Command::new("xattr")
            .args(["-w", name, value])
            .arg(&photo)
            .status();
        assert!(ok.unwrap().success());
    }

    let outcome = run(Kind::Copy, vec![folder], &disk.mount);

    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    let message = outcome.failures[0].error.to_string();
    assert!(
        message.contains("too large for the volume's format"),
        "{message}"
    );
    let landed = disk.mount.join("Folder");
    assert_eq!(fs::read(landed.join("photo.jpg")).unwrap(), b"photo");
    assert!(
        !taken(&landed.join(".DS_Store")),
        "Finder's view settings reached FAT"
    );
    assert!(!taken(&landed.join("huge.mov")) && staged_in(&landed).is_empty());
    let names = Command::new("xattr")
        .arg(landed.join("photo.jpg"))
        .output()
        .unwrap();
    let names = String::from_utf8_lossy(&names.stdout);
    assert!(names.contains("_kMDItemUserTags"), "{names}");
    assert!(!names.contains("kMDItemWhereFroms"), "{names}");

    let _ = fs::remove_dir_all(&dir);
}

fn left(bytes: u64, items: u64, items_done: u64, counting: bool) -> Left {
    Left {
        bytes,
        items,
        items_done,
        counting,
    }
}

#[test]
fn the_speed_settles_and_the_time_left_waits_for_the_count() {
    let mut meter = Meter::default();
    let start = Instant::now();
    let rate = 100.0 * 1024.0 * 1024.0;
    let total = 2u64 << 30;
    let at = |secs: f64| start + Duration::from_secs_f64(secs);
    for tick in 0..=40 {
        let secs = f64::from(tick) * 0.25;
        let done = (secs * rate) as u64;
        let counting = tick < 20;
        meter.sample(at(secs), done, false, &left(total - done, 10, 2, counting));
        if counting || secs < 3.0 {
            assert_eq!(meter.eta(), None, "an estimate at {secs}s");
        }
    }
    let speed = meter.bytes_per_sec();
    assert!((speed / rate - 1.0).abs() < 0.01, "{speed}");
    let eta = meter.eta().expect("settled and counted").as_secs_f64();
    assert!(
        (eta - 10.48).abs() < 0.5,
        "1,048 MiB left at 100 MiB/s: {eta}"
    );

    for tick in 41..=60 {
        meter.sample(
            at(f64::from(tick) * 0.25),
            1 << 40,
            true,
            &left(1, 1, 2, false),
        );
    }
    let paused = meter.bytes_per_sec();
    assert!(
        (paused / rate - 1.0).abs() < 0.01,
        "a pause read as a stall"
    );
}

#[test]
fn the_wait_for_the_first_byte_does_not_read_as_a_slow_start() {
    let mut meter = Meter::default();
    let start = Instant::now();
    let rate = 100.0 * 1024.0 * 1024.0;
    for tick in 0..=8 {
        let secs = f64::from(tick) * 0.25;
        let done = ((secs - 0.6).max(0.0) * rate) as u64;
        let at = start + Duration::from_secs_f64(secs);
        meter.sample(at, done, false, &left(1 << 40, 10, 2, true));
        let speed = meter.bytes_per_sec();
        assert!(
            speed == 0.0 || (speed / rate - 1.0).abs() < 0.01,
            "{speed} at {secs}s"
        );
    }
    assert!(meter.bytes_per_sec() > 0.0, "no speed once copying");
}

#[test]
fn many_small_files_are_timed_by_the_item() {
    let mut meter = Meter::default();
    let start = Instant::now();
    for tick in 0..=20 {
        let secs = f64::from(tick) * 0.25;
        let done = (secs * 1e6) as u64;
        let items_done = (secs * 100.0) as u64;
        meter.sample(
            start + Duration::from_secs_f64(secs),
            done,
            false,
            &left(1_000, 5_000, items_done, false),
        );
    }
    let eta = meter.eta().expect("settled").as_secs_f64();
    assert!(
        eta > 40.0,
        "5,000 items at 100 a second, not 1 KB at 1 MB/s: {eta}"
    );
}

#[test]
fn a_file_whose_access_list_denies_its_deletion_still_copies() {
    let Some(disk) = RamDisk::new(40, "HFS+", &volume_name("A")) else {
        return;
    };
    let dir = temp_dir("acl");
    let file = dir.join("keep.txt");
    write(&file, b"keep");
    let denied = Command::new("chmod")
        .args(["+a", "everyone deny delete"])
        .arg(&file)
        .status()
        .is_ok_and(|status| status.success());
    assert!(denied, "chmod +a");

    let outcome = run(Kind::Copy, vec![file.clone()], &disk.mount);
    let litter = staged_in(&disk.mount);
    let landed = fs::read(disk.mount.join("keep.txt"));
    let _ = Command::new("chmod").args(["-N"]).arg(&file).status();
    let _ = Command::new("chmod")
        .args(["-N"])
        .arg(disk.mount.join("keep.txt"))
        .status();
    let _ = fs::remove_dir_all(&dir);

    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(landed.unwrap(), b"keep");
    assert_eq!(litter, Vec::<PathBuf>::new());
}

#[test]
fn a_copy_keeps_the_creation_date() {
    use std::os::macos::fs::MetadataExt as _;
    let Some(disk) = RamDisk::new(40, "HFS+", &volume_name("B")) else {
        return;
    };
    let dir = temp_dir("birth");
    write(&dir.join("Folder").join("old.txt"), b"old");
    thread::sleep(Duration::from_millis(2100));

    let outcome = run(Kind::Copy, vec![dir.join("Folder")], &disk.mount);
    let born = |path: &Path| fs::metadata(path).unwrap().st_birthtime();
    let (file, folder) = (dir.join("Folder").join("old.txt"), dir.join("Folder"));
    let kept = (
        born(&disk.mount.join("Folder").join("old.txt")) == born(&file),
        born(&disk.mount.join("Folder")) == born(&folder),
    );
    let _ = fs::remove_dir_all(&dir);

    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(
        kept,
        (true, true),
        "file and folder keep their creation dates"
    );
}

#[test]
fn a_merge_replaces_a_different_file_of_the_same_size_and_date() {
    if !crate::trash_reachable() {
        return;
    }
    let dir = temp_dir("merge-alike");
    let (src, dst) = (dir.join("src").join("Folder"), dir.join("dst"));
    write(&src.join("x.txt"), b"new!");
    write(&dst.join("Folder").join("x.txt"), b"old!");
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for path in [src.join("x.txt"), dst.join("Folder").join("x.txt")] {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    let outcome = Transfer::default().run(Request::new(
        Kind::Copy,
        vec![src],
        &dst,
        Conflict::Merge,
        OnError::Continue,
    ));
    let merged = fs::read(dst.join("Folder").join("x.txt")).unwrap();
    let displaced: Vec<PathBuf> = match &outcome.done.undo {
        Some(Op::PutBack(back)) => back.iter().map(|restore| restore.item.clone()).collect(),
        Some(Op::Batch(steps)) => steps
            .iter()
            .filter_map(|step| match step {
                Op::PutBack(back) => Some(back.iter().map(|restore| restore.item.clone())),
                _ => None,
            })
            .flatten()
            .collect(),
        _ => Vec::new(),
    };
    purge(&displaced);
    let _ = fs::remove_dir_all(&dir);

    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(merged, b"new!", "same size and date, different bytes");
}

#[test]
fn jobs_started_together_into_one_folder_each_land_under_their_own_name() {
    let dir = temp_dir("together");
    let dst = dir.join("dst");
    fs::create_dir_all(&dst).unwrap();
    let jobs = 8;
    let sources: Vec<PathBuf> = (0..jobs).map(|i| dir.join(format!("src{i}"))).collect();
    for (i, src) in sources.iter().enumerate() {
        for n in 0..20 {
            write(
                &src.join("Set").join(format!("{n}.txt")),
                format!("{i}").as_bytes(),
            );
        }
        write(&src.join("same.txt"), format!("{i}").as_bytes());
    }

    let start = std::sync::Barrier::new(jobs);
    let outcomes: Vec<Outcome> = thread::scope(|scope| {
        let handles: Vec<_> = sources
            .iter()
            .map(|src| {
                let (start, dst) = (&start, &dst);
                scope.spawn(move || {
                    let items = vec![src.join("Set"), src.join("same.txt")];
                    start.wait();
                    run(Kind::Copy, items, dst)
                })
            })
            .collect();
        handles.into_iter().map(|job| job.join().unwrap()).collect()
    });
    let mut sets = Vec::new();
    let mut files = Vec::new();
    for entry in fs::read_dir(&dst).unwrap().flatten() {
        let path = entry.path();
        match path.is_dir() {
            true => sets.push(fs::read(path.join("0.txt")).unwrap()),
            false => files.push(fs::read(&path).unwrap()),
        }
    }
    sets.sort();
    files.sort();
    let _ = fs::remove_dir_all(&dir);

    for outcome in &outcomes {
        assert_eq!(outcome.end, End::Finished, "{:?}", outcome.failures);
    }
    let each: Vec<Vec<u8>> = (0..jobs).map(|i| format!("{i}").into_bytes()).collect();
    assert_eq!(
        sets, each,
        "every job's folder landed under a name of its own"
    );
    assert_eq!(
        files, each,
        "every job's file landed under a name of its own"
    );
}

#[test]
fn a_name_taken_after_the_plan_is_kept_whatever_the_conflict_choice() {
    let Some(disk) = RamDisk::new(160, "HFS+", &volume_name("T")) else {
        return;
    };
    let dir = temp_dir("taken-later");
    let big = dir.join("big.bin");
    sparse(&big, 64 << 20);

    let transfer = Transfer::default();
    let replace = Request::new(
        Kind::Copy,
        vec![big],
        &disk.mount,
        Conflict::Replace,
        OnError::Continue,
    );
    let handle = held_mid_file(&transfer, replace);
    write(&disk.mount.join("big.bin"), b"theirs");
    transfer.resume();
    let outcome = handle.join().unwrap();

    assert_eq!(outcome.end, End::Finished, "{:?}", outcome.failures);
    assert_eq!(fs::read(disk.mount.join("big.bin")).unwrap(), b"theirs");
    assert_eq!(outcome.done.results, vec![disk.mount.join("big 2.bin")]);
    assert_eq!(
        fs::metadata(disk.mount.join("big 2.bin")).unwrap().len(),
        64 << 20
    );

    let _ = fs::remove_dir_all(&dir);
}
