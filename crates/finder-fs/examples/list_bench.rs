use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

fn make_files(dir: &Path, count: usize) {
    fs::create_dir_all(dir).unwrap();
    for i in 0..count {
        fs::write(dir.join(format!("file-{i:06}.txt")), b"x").unwrap();
    }
}

fn time_it(f: impl Fn()) -> Duration {
    f();
    let mut runs: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    runs.sort();
    runs[runs.len() / 2]
}

fn read_dir_baseline(dir: &Path) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let _ = entry.metadata().unwrap();
    }
}

fn bench(dir: &Path, count: usize) {
    let bulk = time_it(|| {
        finder_fs::list::list_dir(dir).unwrap();
    });
    let baseline = time_it(|| read_dir_baseline(dir));
    println!(
        "{count:>7} files: list_dir(getattrlistbulk) = {:>8.2} ms   read_dir+metadata = {:>8.2} ms",
        bulk.as_secs_f64() * 1000.0,
        baseline.as_secs_f64() * 1000.0
    );
}

fn main() {
    let root = std::env::temp_dir().join("finder-fs-list-bench");
    let _ = fs::remove_dir_all(&root);

    for count in [20_000usize, 100_000] {
        let dir = root.join(count.to_string());
        make_files(&dir, count);
        bench(&dir, count);
    }

    fs::remove_dir_all(&root).unwrap();
}
