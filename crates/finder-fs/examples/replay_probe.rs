use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let since: u64 = args
        .next()
        .expect("usage: replay_probe <event id> root...")
        .parse()
        .unwrap();
    let roots: Vec<PathBuf> = args.map(PathBuf::from).collect();
    let (tx, rx) = mpsc::channel();
    let _watcher = finder_fs::watch::since(since, roots, move |replay| {
        let _ = tx.send(replay);
    });
    let start = Instant::now();
    let (mut batches, mut dirs, mut rescans, mut wrapped, mut reached) = (0, 0, 0, false, since);
    while let Ok(replay) = rx.recv_timeout(Duration::from_secs(30)) {
        batches += 1;
        dirs += replay.dirs.len();
        rescans += replay.rescan.len();
        wrapped |= replay.wrapped;
        reached = reached.max(replay.event_id);
        if !replay.rescan.is_empty() && rescans <= 10 {
            println!(
                "rescan at {:.1}s: {:?}",
                start.elapsed().as_secs_f64(),
                replay.rescan
            );
        }
        if replay.history_done {
            println!("history done at {:.1}s", start.elapsed().as_secs_f64());
            break;
        }
    }
    println!(
        "{:.1}s  batches {batches}  dirs {dirs}  rescans {rescans}  wrapped {wrapped}  reached {reached}",
        start.elapsed().as_secs_f64()
    );
}
