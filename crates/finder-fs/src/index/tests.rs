use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Index, Progress};

const MANY: usize = 2000;
const DEEP_LEVELS: usize = 300;
const MANY_BYTES: u64 = (MANY * 10) as u64;
const DEEP_BYTES: u64 = 4 + 40 + 400;
const APP_BYTES: u64 = 500 + 60;
const SETTLE: Duration = Duration::from_millis(800);

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.0.join("locked"), fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tree(name: &str) -> Temp {
    let root = std::env::temp_dir().join(format!("finder-fs-index-{name}"));
    let _ = fs::remove_dir_all(&root);
    let deep = root.join("a/b/c");
    fs::create_dir_all(&deep).unwrap();
    fs::write(root.join("a/one.txt"), vec![b'x'; 4]).unwrap();
    fs::write(root.join("a/b/two.txt"), vec![b'x'; 40]).unwrap();
    fs::write(deep.join("three.txt"), vec![b'x'; 400]).unwrap();
    fs::write(root.join(".hidden"), b"").unwrap();

    let many = root.join("many");
    fs::create_dir_all(&many).unwrap();
    for i in 0..MANY {
        fs::write(many.join(format!("file{i:04}.dat")), vec![b'y'; 10]).unwrap();
    }

    let app = root.join("Demo.app/Contents/MacOS");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("Demo"), vec![b'z'; 500]).unwrap();
    fs::write(root.join("Demo.app/Contents/Info.plist"), vec![b'z'; 60]).unwrap();

    fs::create_dir_all(root.join("ring")).unwrap();
    symlink(root.join("ring"), root.join("ring/back")).unwrap();

    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("secret.txt"), vec![b'q'; 1000]).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    Temp(fs::canonicalize(&root).unwrap())
}

fn crawled(root: &Path) -> Arc<Index> {
    let index = Index::open(&store(
        &root.file_name().unwrap_or_default().to_string_lossy(),
    ));
    crawl(&index, root);
    index
}

fn sync(index: &Arc<Index>, root: &Path) {
    let (tx, rx) = mpsc::channel();
    index.sync(vec![root.to_path_buf()], move |progress: Progress| {
        if progress.done {
            let _ = tx.send(());
        }
    });
    rx.recv_timeout(Duration::from_secs(120)).unwrap();
}

fn store(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("finder-fs-store-{name}.bin"));
    let _ = fs::remove_file(&path);
    path
}

fn reads_under(root: &Path, mark: usize) -> Vec<PathBuf> {
    crate::list::READS.lock().unwrap()[mark..]
        .iter()
        .filter(|dir| dir.starts_with(root))
        .cloned()
        .collect()
}

fn reads_mark() -> usize {
    crate::list::READS.lock().unwrap().len()
}

fn crawl(index: &Arc<Index>, root: &Path) {
    let (tx, rx) = mpsc::channel();
    index.crawl(vec![root.to_path_buf()], move |progress: Progress| {
        if progress.done {
            let _ = tx.send(());
        }
    });
    rx.recv_timeout(Duration::from_secs(120)).unwrap();
}

fn names(index: &Index, dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = index
        .children(dir)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    names.sort();
    names
}

#[test]
fn children_and_sizes() {
    let temp = tree("children");
    let root = &temp.0;
    let index = crawled(root);

    assert_eq!(
        names(&index, root),
        [".hidden", "Demo.app", "a", "locked", "many", "ring"]
    );
    assert_eq!(index.children(&root.join("many")).unwrap().len(), MANY);
    assert_eq!(names(&index, &root.join("a")), ["b", "one.txt"]);
    assert!(index.children(&root.join("nope")).is_none());

    assert_eq!(index.folder_size(&root.join("many")), Some(MANY_BYTES));
    assert_eq!(index.folder_size(&root.join("a")), Some(DEEP_BYTES));
    assert_eq!(index.folder_size(&root.join("a/b/c")), Some(400));

    assert!(index.children(&root.join("Demo.app")).is_none());
    assert_eq!(index.folder_size(&root.join("Demo.app")), Some(APP_BYTES));
    let app = index
        .children(root)
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "Demo.app")
        .unwrap();
    assert!(app.package && app.size == APP_BYTES);

    assert_eq!(names(&index, &root.join("ring")), ["back"]);
    let hits = index.search("back", None, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry.kind, crate::EntryKind::Symlink);

    assert!(index.children(&root.join("locked")).is_none());
    assert_eq!(index.folder_size(&root.join("locked")), Some(0));
    let total = index.folder_size(root).unwrap();
    assert!(total >= MANY_BYTES + DEEP_BYTES + APP_BYTES);
    assert!(total < MANY_BYTES + DEEP_BYTES + APP_BYTES + 1000);

    let stats = index.stats();
    assert_eq!(stats.files as usize, MANY + 6);
    let above = root.components().count() as u64 - 2;
    assert_eq!(stats.dirs, above + 7);
    assert!(stats.crawled_at.is_some());

    crawl(&index, root);
    let again = index.stats();
    assert_eq!(
        (again.dirs, again.files, again.bytes),
        (stats.dirs, stats.files, stats.bytes)
    );
}

#[test]
fn a_second_crawl_is_ignored() {
    let temp = tree("second");
    let root = &temp.0;
    let index = Index::open(&store("second"));
    let (first, done) = mpsc::channel();
    index.crawl(vec![root.clone()], move |progress: Progress| {
        if progress.done {
            let _ = first.send(());
        }
    });
    let (second, ignored) = mpsc::channel();
    index.crawl(vec![root.join("a")], move |progress: Progress| {
        let _ = second.send(progress.done);
    });
    done.recv_timeout(Duration::from_secs(120)).unwrap();
    assert!(ignored.try_recv().is_err());
    assert_eq!(index.stats().files as usize, MANY + 6);
}

#[test]
fn search_ranks_and_scopes() {
    let temp = tree("search");
    let root = &temp.0;
    let index = crawled(root);

    let hits = index.search("one", None, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, root.join("a/one.txt"));
    assert_eq!(hits[0].entry.size, 4);

    let dir = root.join("rank");
    fs::create_dir_all(dir.join("deep")).unwrap();
    fs::write(dir.join("target.txt"), b"").unwrap();
    fs::write(dir.join("my-target.txt"), b"").unwrap();
    fs::write(dir.join("mytarget.txt"), b"").unwrap();
    fs::write(dir.join("deep/target.txt"), b"").unwrap();
    crawl(&index, root);

    let hits = index.search("TARGET", None, 10);
    let order: Vec<&str> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
    assert_eq!(
        order,
        ["target.txt", "target.txt", "my-target.txt", "mytarget.txt"]
    );
    assert_eq!(hits[0].path, dir.join("target.txt"));
    assert_eq!(hits[1].path, dir.join("deep/target.txt"));
    assert!(hits[0].score > hits[2].score);

    assert_eq!(index.search("target", Some(&dir.join("deep")), 10).len(), 1);
    assert_eq!(index.search("target", Some(&root.join("a")), 10).len(), 0);
    assert_eq!(index.search("file", Some(&root.join("many")), 7).len(), 7);
    assert!(index.search("", None, 10).is_empty());
    assert!(
        index
            .search("target", Some(Path::new("/nowhere")), 10)
            .is_empty()
    );
    assert!(index.search("Info.plist", None, 10).is_empty());
}

#[test]
fn a_hit_below_a_byte_of_depth_carries_its_real_path() {
    let root = std::env::temp_dir().join("finder-fs-index-depth");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let temp = Temp(fs::canonicalize(&root).unwrap());
    let mut leaf = temp.0.clone();
    for _ in 0..DEEP_LEVELS {
        leaf.push("d");
    }
    fs::create_dir_all(&leaf).unwrap();
    fs::write(leaf.join("needle.txt"), b"x").unwrap();
    let index = crawled(&temp.0);

    let hits = index.search("needle", None, 4);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, leaf.join("needle.txt"));
    assert!(hits[0].path.exists());
    assert_eq!(index.search("needle", Some(&temp.0), 4).len(), 1);
    assert_eq!(index.children(&leaf).unwrap()[0].name, "needle.txt");
}

fn dated(dir: &Path, name: &str, at: SystemTime) {
    let file = fs::File::create(dir.join(name)).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(at))
        .unwrap();
}

#[test]
fn a_date_outside_the_unsigned_epoch_reads_alike_from_the_index_and_the_disk() {
    let root = std::env::temp_dir().join("finder-fs-index-dates");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let temp = Temp(fs::canonicalize(&root).unwrap());
    let before_epoch = UNIX_EPOCH - Duration::from_secs(14_182_940);
    let reference = UNIX_EPOCH + Duration::from_secs(978_307_200);
    let next_century = UNIX_EPOCH + Duration::from_secs(4_102_444_800);
    let far_future = UNIX_EPOCH + Duration::from_secs(7_258_118_400);
    dated(&temp.0, "pre-epoch-1969.txt", before_epoch);
    dated(&temp.0, "reference-2001.txt", reference);
    dated(&temp.0, "future-2100.txt", next_century);
    dated(&temp.0, "year-2200.txt", far_future);
    let store = store("dates");
    let index = Index::open(&store);
    crawl(&index, &temp.0);

    let dates = |entries: Vec<crate::Entry>| {
        let mut dates: Vec<(String, SystemTime)> = entries
            .into_iter()
            .map(|entry| (entry.name, entry.modified))
            .collect();
        dates.sort();
        dates
    };
    let from_disk = dates(crate::list::list_dir(&temp.0).unwrap());
    assert_eq!(
        from_disk,
        [
            ("future-2100.txt".to_string(), next_century),
            ("pre-epoch-1969.txt".to_string(), before_epoch),
            ("reference-2001.txt".to_string(), reference),
            ("year-2200.txt".to_string(), far_future),
        ]
    );
    assert_eq!(dates(index.children(&temp.0).unwrap()), from_disk);

    index.save();
    assert_eq!(
        dates(Index::open(&store).children(&temp.0).unwrap()),
        from_disk
    );
}

#[test]
fn search_folds_case_diacritics_and_composition() {
    let temp = tree("fold");
    let root = &temp.0;
    fs::write(root.join("cafe\u{301}-nfd.txt"), b"").unwrap();
    fs::write(root.join("caf\u{e9}-nfc.txt"), b"").unwrap();
    let index = crawled(root);

    for query in ["cafe", "caf\u{e9}", "cafe\u{301}", "CAF\u{c9}"] {
        let mut found: Vec<String> = index
            .search(query, None, 10)
            .into_iter()
            .map(|hit| hit.entry.name)
            .collect();
        found.sort();
        assert_eq!(
            found,
            ["cafe\u{301}-nfd.txt", "caf\u{e9}-nfc.txt"],
            "searching {query:?}"
        );
    }
}

#[test]
fn note_changed_moves_sizes_up() {
    let temp = tree("changed");
    let root = &temp.0;
    let index = crawled(root);
    let before = index.folder_size(root).unwrap();

    fs::write(root.join("many/added.dat"), vec![b'n'; 777]).unwrap();
    fs::remove_file(root.join("many/file0000.dat")).unwrap();
    index.note_changed(&[root.join("many")]);

    assert_eq!(index.children(&root.join("many")).unwrap().len(), MANY);
    assert_eq!(
        index.folder_size(&root.join("many")),
        Some(MANY_BYTES + 777 - 10)
    );
    assert_eq!(index.folder_size(root), Some(before + 777 - 10));
    assert_eq!(index.search("added", None, 4).len(), 1);
    assert_eq!(index.search("file0000", None, 4).len(), 0);

    fs::remove_dir_all(root.join("a/b")).unwrap();
    index.note_changed(&[root.join("a/b"), root.join("a")]);
    assert_eq!(index.folder_size(&root.join("a")), Some(4));
    assert!(index.search("three", None, 4).is_empty());
    assert_eq!(index.folder_size(root), Some(before + 777 - 10 - 440));
}

#[test]
fn snapshot_round_trip() {
    let temp = tree("snapshot");
    let root = &temp.0;
    let store = root.join("cache/index.bin");
    let index = Index::open(&store);
    crawl(&index, root);
    index.save();

    let reopened = Index::open(&store);
    let (before, after) = (index.stats(), reopened.stats());
    assert_eq!(
        (before.dirs, before.files, before.bytes),
        (after.dirs, after.files, after.bytes)
    );
    assert_eq!(
        before.crawled_at.map(super::snapshot::epoch),
        after.crawled_at.map(super::snapshot::epoch)
    );
    assert_eq!(names(&reopened, root), names(&index, root));
    assert_eq!(reopened.children(&root.join("many")).unwrap().len(), MANY);
    assert_eq!(reopened.folder_size(&root.join("a")), Some(DEEP_BYTES));
    assert_eq!(
        reopened.folder_size(&root.join("Demo.app")),
        Some(APP_BYTES)
    );
    assert_eq!(reopened.search("three", None, 4).len(), 1);
    assert!(reopened.children(&root.join("Demo.app")).is_none());

    fs::write(&store, b"NGVIDX03 and nothing else").unwrap();
    assert_eq!(Index::open(&store).stats().files, 0);
}

#[test]
fn a_cold_sync_crawls_the_whole_tree() {
    let temp = tree("cold-sync");
    let root = &temp.0;
    let store = store("cold-sync");
    let index = Index::open(&store);
    let mark = reads_mark();

    sync(&index, root);

    assert_eq!(index.children(&root.join("many")).unwrap().len(), MANY);
    assert_eq!(index.stats().files as usize, MANY + 6);
    assert!(reads_under(root, mark).len() >= 8);
    let _ = fs::remove_file(&store);
}

#[test]
fn a_warm_sync_over_an_unchanged_tree_reads_nothing() {
    let temp = tree("warm-sync");
    let root = &temp.0;
    let store = store("warm-sync");
    let index = Index::open(&store);
    // FSEvents logs the fixture's writes a moment after they return.
    std::thread::sleep(SETTLE);
    sync(&index, root);
    index.save();
    drop(index);
    std::thread::sleep(SETTLE);

    let index = Index::open(&store);
    let mark = reads_mark();
    sync(&index, root);

    assert_eq!(reads_under(root, mark), Vec::<PathBuf>::new());
    assert_eq!(index.stats().files as usize, MANY + 6);
    assert_eq!(index.children(&root.join("many")).unwrap().len(), MANY);
    assert_eq!(index.folder_size(&root.join("a")), Some(DEEP_BYTES));
    let _ = fs::remove_file(&store);
}

#[test]
fn a_warm_sync_reads_the_subtree_that_changed() {
    let temp = tree("changed-sync");
    let root = &temp.0;
    let store = store("changed-sync");
    let index = Index::open(&store);
    std::thread::sleep(SETTLE);
    sync(&index, root);
    index.save();
    drop(index);
    std::thread::sleep(SETTLE);

    fs::write(root.join("a/b/c/four.txt"), vec![b'x'; 4000]).unwrap();
    fs::create_dir_all(root.join("a/b/c/fresh")).unwrap();
    fs::write(root.join("a/b/c/fresh/five.txt"), vec![b'x'; 50]).unwrap();
    std::thread::sleep(SETTLE);

    let index = Index::open(&store);
    let mark = reads_mark();
    sync(&index, root);

    let read = reads_under(root, mark);
    assert!(read.contains(&root.join("a/b/c")), "{read:?}");
    assert!(!read.contains(&root.join("many")), "{read:?}");
    assert_eq!(index.search("four", None, 4).len(), 1);
    assert_eq!(index.search("five", None, 4).len(), 1);
    assert_eq!(
        index.folder_size(&root.join("a")),
        Some(DEEP_BYTES + 4000 + 50)
    );
    assert_eq!(index.children(&root.join("many")).unwrap().len(), MANY);
    let _ = fs::remove_file(&store);
}

#[test]
fn an_event_id_the_log_cannot_hold_forces_a_full_crawl() {
    let temp = tree("stale-id");
    let root = &temp.0;
    let store = store("stale-id");
    let index = Index::open(&store);
    sync(&index, root);

    let tree = index.read();
    let roots = std::slice::from_ref(root);
    let now = crate::watch::latest_event_id();
    assert_eq!(super::catchup::plan(&tree, roots, now).warm, roots);

    let plan = super::catchup::plan(&tree, roots, 1);
    assert_eq!(plan.cold, roots);
    assert!(plan.warm.is_empty() && plan.since_when == 0);

    let other = vec![root.join("a")];
    assert_eq!(super::catchup::plan(&tree, &other, now).cold, other);
    drop(tree);
    let _ = fs::remove_file(&store);
}

#[test]
fn a_finished_crawl_lands_a_snapshot() {
    let temp = tree("crawl-save");
    let root = &temp.0;
    let store = store("crawl-save");
    let index = Index::open(&store);
    crawl(&index, root);

    let reopened = Index::open(&store);
    assert_eq!(reopened.stats().files, index.stats().files);
    assert_eq!(reopened.stats().event_id, index.stats().event_id);
    assert_eq!(reopened.children(&root.join("many")).unwrap().len(), MANY);
    let _ = fs::remove_file(&store);
}

#[test]
fn a_snapshot_keeps_the_id_of_the_tree_it_holds() {
    let temp = tree("older-id");
    let root = &temp.0;
    let store = store("older-id");
    let index = Index::open(&store);
    crawl(&index, root);
    let landed = index.stats().event_id;

    fs::write(root.join("many/late.dat"), vec![b'l'; 90]).unwrap();
    std::thread::sleep(SETTLE);
    index.note_changed(&[root.join("many")]);
    index.save();

    let reopened = Index::open(&store);
    assert_eq!(reopened.search("late", None, 4).len(), 1);
    assert_eq!(reopened.stats().event_id, landed);
    assert!(landed <= crate::watch::latest_event_id());
    let _ = fs::remove_file(&store);
}

#[test]
fn a_failed_write_leaves_the_snapshot_that_landed() {
    let temp = tree("atomic");
    let root = &temp.0;
    let store = store("atomic");
    let index = Index::open(&store);
    crawl(&index, root);
    let landed = fs::read(&store).unwrap();

    let temp_path = store.with_extension("tmp");
    let _ = fs::remove_file(&temp_path);
    fs::create_dir_all(&temp_path).unwrap();
    fs::write(root.join("late.txt"), vec![b'l'; 5]).unwrap();
    index.note_changed(&[root.clone()]);
    index.save();

    assert_eq!(fs::read(&store).unwrap(), landed);
    assert_eq!(Index::open(&store).search("late", None, 4).len(), 0);

    fs::remove_dir(&temp_path).unwrap();
    index.save();
    assert_eq!(Index::open(&store).search("late", None, 4).len(), 1);
    let _ = fs::remove_file(&store);
}

#[test]
fn visible_children_skips_the_hidden_and_the_unread() {
    let temp = tree("visible");
    let root = &temp.0;
    let index = crawled(root);

    assert_eq!(index.visible_children(root), Some(5));
    assert_eq!(index.visible_children(&root.join("a/b/c")), Some(1));
    assert_eq!(index.visible_children(&root.join("locked")), None);
    assert_eq!(index.visible_children(&root.join("nowhere")), None);
}

#[test]
fn a_reopened_snapshot_drops_what_was_deleted() {
    let temp = tree("compact");
    let root = &temp.0;
    let store = store("compact");
    let index = Index::open(&store);
    crawl(&index, root);
    let whole = fs::metadata(&store).unwrap().len();

    fs::remove_dir_all(root.join("many")).unwrap();
    index.note_changed(&[root.join("many"), root.clone()]);
    index.save();

    let index = Index::open(&store);
    assert_eq!(
        names(&index, root),
        [".hidden", "Demo.app", "a", "locked", "ring"]
    );
    assert_eq!(names(&index, &root.join("a/b/c")), ["three.txt"]);
    assert!(index.children(&root.join("many")).is_none());
    assert_eq!(index.search("file0001", None, 4).len(), 0);
    assert_eq!(index.search("three", None, 4).len(), 1);
    assert_eq!(index.folder_size(&root.join("a")), Some(DEEP_BYTES));

    index.note_changed(&[root.clone()]);
    index.save();
    assert!(fs::metadata(&store).unwrap().len() < whole);
    let _ = fs::remove_file(&store);
}

#[test]
fn search_ranks_hidden_files_after_shown_ones() {
    let temp = tree("hidden-search");
    let root = &temp.0;
    let index = crawled(root);

    let dotted = root.join(".config/inner");
    fs::create_dir_all(&dotted).unwrap();
    fs::write(dotted.join("marker.txt"), b"").unwrap();
    fs::write(root.join(".marker"), b"").unwrap();
    fs::write(root.join("marker.txt"), b"").unwrap();
    crawl(&index, root);

    let hits = index.search("marker", None, 10);
    let found: Vec<&Path> = hits.iter().map(|hit| hit.path.as_path()).collect();
    assert_eq!(
        found,
        [
            root.join("marker.txt").as_path(),
            dotted.join("marker.txt").as_path(),
            root.join(".marker").as_path(),
        ]
    );
}

#[test]
fn search_takes_every_word_and_puts_the_whole_name_first() {
    let temp = tree("words-search");
    let root = &temp.0;
    let index = crawled(root);

    fs::write(root.join("report.txt"), b"").unwrap();
    fs::write(root.join("report-of-the-year.txt"), b"").unwrap();
    fs::write(root.join("year-of-the-donkey.txt"), b"").unwrap();
    crawl(&index, root);

    let order: Vec<String> = index
        .search("report", None, 10)
        .iter()
        .map(|hit| hit.entry.name.clone())
        .collect();
    assert_eq!(order, ["report.txt", "report-of-the-year.txt"]);

    let order: Vec<String> = index
        .search("report year", None, 10)
        .iter()
        .map(|hit| hit.entry.name.clone())
        .collect();
    assert_eq!(order, ["report-of-the-year.txt"]);
    assert!(index.search("report donkey", None, 10).is_empty());
}

#[test]
fn a_file_added_after_a_search_is_found_by_the_next_one() {
    let temp = tree("tail-search");
    let root = &temp.0;
    let index = crawled(root);
    assert_eq!(index.search("afterwards", None, 10).len(), 0);

    fs::write(root.join("afterwards.txt"), b"").unwrap();
    index.note_changed(&[root.clone()]);

    let hits = index.search("afterwards", None, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, root.join("afterwards.txt"));
}

fn eventually(check: impl Fn() -> bool) -> bool {
    (0..50).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        check()
    })
}

#[test]
fn a_followed_index_keeps_up_with_the_disk() {
    let temp = tree("follow");
    let root = &temp.0;
    let index = crawled(root);
    let _following = index.follow(vec![root.clone()]);
    let found = |name: &str| index.search(name, None, 10).len();

    fs::write(root.join("a/b/fresh.txt"), b"").unwrap();
    assert!(eventually(|| found("fresh") == 1));

    fs::create_dir(root.join("newdir")).unwrap();
    fs::write(root.join("newdir/inside.txt"), b"").unwrap();
    assert!(eventually(|| found("inside") == 1));

    fs::remove_file(root.join("a/b/fresh.txt")).unwrap();
    assert!(eventually(|| found("fresh") == 0));
}
