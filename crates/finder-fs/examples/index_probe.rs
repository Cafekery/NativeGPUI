use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use finder_fs::index::Index;

fn cpu() -> Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let secs = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    secs(usage.ru_utime) + secs(usage.ru_stime)
}

fn footprint() -> (f64, f64) {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V4,
            std::ptr::addr_of_mut!(info) as *mut _,
        )
    };
    let mb = |bytes: u64| bytes as f64 / 1_048_576.0;
    (
        mb(info.ri_phys_footprint),
        mb(info.ri_lifetime_max_phys_footprint),
    )
}

fn line(step: &str, start: Instant, extra: String) {
    let (now, peak) = footprint();
    println!(
        "{step:<7} {:>8.2} s  cpu {:>7.2} s  footprint {:>6.0} MB  peak {:>6.0} MB  {extra}",
        start.elapsed().as_secs_f64(),
        cpu().as_secs_f64(),
        now,
        peak
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let store = PathBuf::from(
        args.next()
            .expect("usage: index_probe <index.bin> [sync|crawl root...]"),
    );
    let mode = args.next();
    let roots: Vec<PathBuf> = args.map(PathBuf::from).collect();

    let start = Instant::now();
    let index = Index::open(&store);
    let stats = index.stats();
    line(
        "load",
        start,
        format!("{} entries", stats.dirs + stats.files),
    );

    if mode.as_deref() == Some("follow") {
        let _watcher = index.follow(roots);
        let seconds: u64 = std::env::var("NG_FOLLOW_SECS").map_or(60, |s| s.parse().unwrap());
        let start = Instant::now();
        for _ in 0..seconds / 5 {
            std::thread::sleep(Duration::from_secs(5));
            let stats = index.stats();
            line(
                "follow",
                start,
                format!("{} entries", stats.dirs + stats.files),
            );
        }
        return;
    }

    if let Some(mode) = mode {
        let (tx, rx) = mpsc::channel();
        let report = move |progress: finder_fs::index::Progress| {
            if progress.done {
                let _ = tx.send(());
            }
        };
        let start = Instant::now();
        match mode.as_str() {
            "crawl" => index.crawl(roots, report),
            _ => index.sync(roots, report),
        }
        rx.recv().unwrap();
        let stats = index.stats();
        line(
            &mode,
            start,
            format!("{} entries", stats.dirs + stats.files),
        );
        let start = Instant::now();
        index.save();
        line("save", start, String::new());
    }

    let start = Instant::now();
    let hits = index.search("readme", None, 50);
    line(
        "search",
        start,
        format!("{} hits, first search builds the lookup", hits.len()),
    );
}
