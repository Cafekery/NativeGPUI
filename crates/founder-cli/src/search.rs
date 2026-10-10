use std::path::PathBuf;
use std::time::{Duration, Instant};

use fsearch::{Engine, Options, Query, walk};
use serde_json::json;

use crate::args::Args;
use crate::out::{self, Out};
use crate::{CANCELLED, OK, Stop, signals};

const READY_POLL: Duration = Duration::from_millis(50);

/// The index folder; one made for this run alone goes away with it.
struct Index {
    dir: PathBuf,
    temporary: bool,
}

impl Drop for Index {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

struct Opened {
    engine: Engine,
    _index: Index,
}

fn open(args: &Args) -> Result<Opened, Stop> {
    let index = match args.path("index") {
        Some(dir) => Index {
            dir,
            temporary: false,
        },
        None => Index {
            dir: std::env::temp_dir().join(format!("founder-index-{}", std::process::id())),
            temporary: true,
        },
    };
    let home = match args.path("home") {
        Some(home) => home,
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Stop::Usage("no HOME; pass --home".into()))?,
    };
    let engine = Engine::start(Options {
        dir: index.dir.clone(),
        home: home.to_string_lossy().into_owned(),
        skip: None,
    })
    .map_err(Stop::Failed)?;
    Ok(Opened {
        engine,
        _index: index,
    })
}

/// False when Control-C came first.
fn ready(engine: &Engine, out: &mut Out) -> bool {
    signals::install();
    let started = Instant::now();
    let mut told = false;
    while !engine.status().ready {
        if signals::interrupted() {
            return false;
        }
        if !told && started.elapsed() > Duration::from_secs(1) && !out.json {
            out.warn("indexing the disk; the first run of an index folder reads every folder");
            told = true;
        }
        std::thread::sleep(READY_POLL);
    }
    true
}

pub fn search(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let limit = args.number("limit")?;
    let opened = open(&args)?;
    args.done("search")?;
    let words: Vec<String> = (args.rest().iter())
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    if words.is_empty() {
        return Err(Stop::Usage("search needs a query".into()));
    }
    let engine = &opened.engine;
    if !ready(engine, out) {
        return Ok(CANCELLED);
    }
    let mut query = Query::parse(&words.join(" "), engine.home()).map_err(Stop::Usage)?;
    if let Some(limit) = limit {
        query.limit = limit.max(0.) as usize;
    }
    let found = engine.search(&query).map_err(Stop::Failed)?;
    for hit in &found {
        match out.json {
            true => out.emit(json!({
                "event": "found",
                "path": out::path(&hit.path),
                "folder": hit.kind & 3 == walk::KIND_DIR,
                "size": hit.size,
                "modified": hit.mtime,
                "score": hit.score,
            })),
            false => out.say(&hit.path.to_string_lossy()),
        }
    }
    if out.json {
        out.emit(
            json!({ "event": "result", "command": "search", "exit": OK, "count": found.len() }),
        );
    }
    Ok(OK)
}

pub fn totals(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let opened = open(&args)?;
    args.done("totals")?;
    let paths = args.paths();
    if paths.is_empty() {
        return Err(Stop::Usage("totals needs a folder".into()));
    }
    let engine = &opened.engine;
    if !ready(engine, out) {
        return Ok(CANCELLED);
    }
    let mut missing = 0;
    for path in &paths {
        let Some(totals) = engine.totals(path) else {
            missing += 1;
            out.warn(&format!(
                "{}: the index lists no such folder",
                path.display()
            ));
            continue;
        };
        match out.json {
            true => out.emit(json!({
                "event": "totals",
                "path": out::path(path),
                "bytes": totals.bytes,
                "allocated": totals.allocated,
                "items": totals.items,
                "visible": totals.visible,
                "hardlinks": totals.hardlinks,
                "unindexed": totals.unindexed.iter().map(|dir| out::path(dir)).collect::<Vec<_>>(),
            })),
            false => out.say(&format!(
                "{}: {} items, {} ({} on disk), {} shown",
                path.display(),
                totals.items,
                out::size(totals.bytes),
                out::size(totals.allocated),
                totals.visible
            )),
        }
    }
    Ok(match missing {
        0 => OK,
        n if n == paths.len() => crate::REFUSED,
        _ => crate::PARTIAL,
    })
}
