use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use finder_fs::index::Index;

fn cpu() -> Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let secs = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    secs(usage.ru_utime) + secs(usage.ru_stime)
}

fn peak_rss() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // macOS reports `ru_maxrss` in bytes, not the kilobytes the manual page inherited.
    usage.ru_maxrss as u64
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).map_or("/tmp/ngv/fixtures-crawl", |a| a));
    let store = PathBuf::from(args.get(2).map_or("/tmp/ngv/launch-bench.bin", |a| a));
    let mode = args.get(3).map_or("sync", |a| a.as_str());
    if let Some(dir) = args.get(4) {
        let stamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos();
        std::fs::write(Path::new(dir).join(format!("touched{stamp}.dat")), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
    }

    let start = Instant::now();
    let index = Index::open(&store);
    let load = start.elapsed();
    let (tx, rx) = mpsc::channel();
    let report = move |progress: finder_fs::index::Progress| {
        if progress.done {
            let _ = tx.send(());
        }
    };
    match mode {
        "crawl" => index.crawl(vec![root.clone()], report),
        _ => index.sync(vec![root.clone()], report),
    }
    rx.recv().unwrap();
    let wall = start.elapsed();
    let stats = index.stats();
    index.save();
    let on_disk = std::fs::metadata(&store).map_or(0, |meta| meta.len());
    println!(
        "{mode:<6} load {:>6.0} ms  ready {:>7.2} s  cpu {:>7.2} s  peak rss {:>7.1} MB  \
         {} entries  index.bin {:.1} MB",
        load.as_secs_f64() * 1000.0,
        wall.as_secs_f64(),
        cpu().as_secs_f64(),
        mb(peak_rss()),
        stats.dirs + stats.files,
        mb(on_disk),
    );
}
