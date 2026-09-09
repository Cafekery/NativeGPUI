use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use finder_fs::index::Index;

const QUERIES: [&str; 10] = [
    "readme",
    "Info.plist",
    "libssl",
    "index",
    "png",
    "settings",
    "cargo",
    "e",
    "Frameworks",
    "zzzznotfound",
];

fn peak_rss() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // macOS reports `ru_maxrss` in bytes, not the kilobytes the manual page inherited.
    usage.ru_maxrss as u64
}

fn live_rss() -> u64 {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
    unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            std::ptr::addr_of_mut!(info) as *mut libc::c_void,
            size,
        );
    }
    info.pti_resident_size
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn main() {
    let store = std::env::temp_dir().join("finder-fs-index-bench.idx");
    let _ = std::fs::remove_file(&store);
    let mut roots: Vec<PathBuf> = ["/Applications", "/usr", "/Library"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        let developer = Path::new(&home).join("Developer");
        if developer.is_dir() {
            roots.push(developer);
        }
    }
    println!("roots: {roots:?}");

    let index = Index::open(&store);
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    let reader = index.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let watching = std::thread::spawn(move || {
        let (mut memory, mut listing) = (Duration::ZERO, Duration::ZERO);
        while !stopping.load(Ordering::Relaxed) {
            let at = Instant::now();
            reader.stats();
            memory = memory.max(at.elapsed());
            let at = Instant::now();
            reader.children(Path::new("/usr"));
            listing = listing.max(at.elapsed());
            std::thread::sleep(Duration::from_micros(200));
        }
        (memory, listing)
    });
    let listed = AtomicBool::new(false);
    index.crawl(roots.clone(), move |progress| {
        if progress.done {
            let _ = tx.send(());
            return;
        }
        if progress.current.is_none() && !listed.swap(true, Ordering::Relaxed) {
            println!(
                "\rtree       {:>8.2} s  {} entries listed",
                start.elapsed().as_secs_f64(),
                progress.dirs + progress.files
            );
        }
        print!(
            "\r  {:>8} dirs {:>9} files {:>8.1} MB",
            progress.dirs,
            progress.files,
            mb(progress.bytes)
        );
        let _ = std::io::stdout().flush();
    });
    rx.recv().unwrap();
    let crawl = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (memory, listing) = watching.join().unwrap();
    let stats = index.stats();
    let entries = stats.dirs + stats.files;
    println!(
        "\rcrawl      {:>8.2} s  {} entries ({} dirs, {} files), {:.1} GB, {:.0} entries/s",
        crawl.as_secs_f64(),
        entries,
        stats.dirs,
        stats.files,
        mb(stats.bytes) / 1024.0,
        entries as f64 / crawl.as_secs_f64()
    );
    println!(
        "rss        {:>8.1} MB peak, {:.1} MB live, {:.0} bytes/entry",
        mb(peak_rss()),
        mb(live_rss()),
        live_rss() as f64 / entries as f64
    );

    println!(
        "reads      {:>8.2} ms  worst stats(), {:.2} ms worst children(/usr), while crawling",
        memory.as_secs_f64() * 1000.0,
        listing.as_secs_f64() * 1000.0
    );

    let start = Instant::now();
    index.save();
    let save = start.elapsed();
    let on_disk = std::fs::metadata(&store).map_or(0, |meta| meta.len());
    println!(
        "save       {:>8.0} ms  {:.1} MB on disk ({:.0} bytes/entry)",
        save.as_secs_f64() * 1000.0,
        mb(on_disk),
        on_disk as f64 / entries as f64
    );

    for root in &roots {
        println!(
            "  {:<28} {:>8.1} MB  {} children",
            root.display(),
            index.folder_size(root).map_or(f64::NAN, mb),
            index.children(root).map_or(0, |kids| kids.len())
        );
    }

    drop(index);
    let start = Instant::now();
    let index = Index::open(&store);
    let load = start.elapsed();
    println!(
        "load       {:>8.0} ms  {} entries back, {:.1} MB live",
        load.as_secs_f64() * 1000.0,
        index.stats().dirs + index.stats().files,
        mb(live_rss())
    );

    for query in QUERIES {
        let start = Instant::now();
        let hits = index.search(query, None, 50);
        let took = start.elapsed();
        println!(
            "search {:>12}  {:>6.2} ms  {:>2} hits  {}",
            query,
            took.as_secs_f64() * 1000.0,
            hits.len(),
            hits.first().map_or(String::new(), |hit| format!(
                "{} ({})",
                hit.path.display(),
                hit.score
            ))
        );
    }
    let start = Instant::now();
    let scoped = index.search("png", Some(Path::new("/Library")), 50);
    println!(
        "search {:>12}  {:>6.2} ms  {:>2} hits  under /Library",
        "png",
        start.elapsed().as_secs_f64() * 1000.0,
        scoped.len()
    );
}
