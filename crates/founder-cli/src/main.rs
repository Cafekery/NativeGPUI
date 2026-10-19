mod args;
mod journal;
mod listing;
mod marks;
mod out;
mod run;
mod search;
mod signals;
mod viewer;

use std::path::PathBuf;
use std::process::ExitCode;

use finder_fs::ops::transfer::{End, OnError};
use finder_fs::ops::{Conflict, Op, Quick, Restore};
use finder_fs::{list, ops, put_back, queue};
use serde_json::json;

use args::Args;
use journal::{Format, Journal, Policy, Size, Step};
use out::Out;
use run::{Limit, Plan, Ran};

pub const OK: u8 = 0;
pub const PARTIAL: u8 = 1;
pub const REFUSED: u8 = 2;
pub const CANCELLED: u8 = 3;
pub const USAGE: u8 = 64;

pub enum Stop {
    Usage(String),
    Refused(String),
    Failed(String),
}

const HELP: &str = "\
founder: Found'er's file engine from the command line

usage: founder [--json] [--quiet] <command> [options] [arguments]

  ls [-a] [-l] [--sort name|kind|size|modified|created|added] [--reverse] [--folders-first] [path...]
  info [--shallow] path...
  copy [--exactly] source... folder
  move source... folder
  duplicate item...
  trash [--unlock] item...
  delete [--unlock] item...            deletes immediately; no undo
  put-back [--to path] item...         items in a Trash go back where they came from
  rename path name
  new-folder folder [name]
  new-file folder name
  alias item...
  compress item...
  quick rotate-left|rotate-right|pdf|remove-background item...
  quick convert --format jpeg|png|heif [--size actual|large|medium|small] [--metadata] item...
  quick trim item --start seconds --end seconds [--into path]
  tag list|add NAME|remove NAME path...
  comment path [text...] | comment --clear path
  lock path...    unlock path...
  icon set path image | icon remove path...
  search [--index dir] [--home dir] [--limit n] query...
  totals [--index dir] [--home dir] folder...
  undo journal    redo journal    retry journal    journal journal
  viewer status|on|off [--id bundle-id] [--app path]
  clean folder...                      clears what a killed copy or move left: copies in progress
                                       are deleted, items a Replace swapped out go to the Trash

operations (copy through quick, undo, redo, retry):
  --conflict keep-both|replace|merge|skip   when a name is taken (default keep-both)
  --on-error continue|stop                  after a failed item (default continue)
  --journal file                            record the undo, for `founder undo file`
  --pause-after bytes                       test hook: pause there; SIGUSR1 resumes, SIGUSR2 pauses
  --cancel-after bytes|Nitems               test hook: cancel there, as Control-C does

exit: 0 done, 1 partly failed, 2 refused, 3 cancelled, 64 usage
";

fn main() -> ExitCode {
    let args = Args::parse(std::env::args_os().skip(1));
    let mut out = Out::new(false, false);
    let code = args.and_then(|args| {
        out = Out::new(args.flag("json"), args.flag("quiet"));
        dispatch(args, &mut out)
    });
    ExitCode::from(match code {
        Ok(code) => code,
        Err(stop) => stopped(stop, &mut out),
    })
}

fn stopped(stop: Stop, out: &mut Out) -> u8 {
    let (code, message) = match stop {
        Stop::Usage(message) => (USAGE, message),
        Stop::Refused(message) => (REFUSED, message),
        Stop::Failed(message) => (PARTIAL, message),
    };
    if out.json {
        out.emit(json!({ "event": "error", "exit": code, "error": message }));
    } else {
        out.warn(&message);
        if code == USAGE {
            eprintln!("run `founder help` for the commands");
        }
    }
    code
}

fn dispatch(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let command = args.shift().unwrap_or_else(|| "help".into());
    if args.flag("help") || command == "help" {
        print!("{HELP}");
        return Ok(OK);
    }
    match command.as_str() {
        "ls" => listing::ls(args, out),
        "info" => listing::info(args, out),
        "copy" | "move" => transfer(&command, args, out),
        "duplicate" => each(&command, args, out, Op::Duplicate),
        "alias" => each(&command, args, out, Op::MakeAlias),
        "compress" => each(&command, args, out, Op::Compress),
        "trash" | "delete" => remove(&command, args, out),
        "put-back" => put_back(args, out),
        "rename" => rename(args, out),
        "new-folder" | "new-file" => new_item(&command, args, out),
        "quick" => quick(args, out),
        "tag" => marks::tag(args, out),
        "comment" => marks::comment(args, out),
        "lock" => marks::lock(args, true, out),
        "unlock" => marks::lock(args, false, out),
        "icon" => marks::icon(args, out),
        "search" => search::search(args, out),
        "totals" => search::totals(args, out),
        "undo" | "redo" | "retry" => replay(&command, args, out),
        "journal" => show_journal(args, out),
        "viewer" => viewer::viewer(args, out),
        "clean" => clean(args, out),
        other => Err(Stop::Usage(format!("there is no command `{other}`"))),
    }
}

fn plan(args: &Args, conflict: Option<Conflict>) -> Result<Plan, Stop> {
    let conflict = match conflict {
        Some(fixed) => fixed,
        None => match args.text("conflict") {
            None => Conflict::KeepBoth,
            Some(text) => Policy::parse(&text).map(Policy::conflict).ok_or_else(|| {
                Stop::Usage(format!(
                    "--conflict takes keep-both, replace, merge or skip, not {text}"
                ))
            })?,
        },
    };
    let on_error = match args.text("on-error").as_deref() {
        None | Some("continue") => OnError::Continue,
        Some("stop") => OnError::Stop,
        Some(other) => {
            return Err(Stop::Usage(format!(
                "--on-error takes continue or stop, not {other}"
            )));
        }
    };
    let pause_after = match args.text("pause-after") {
        None => None,
        Some(text) => Some(
            args::bytes(&text)
                .ok_or_else(|| Stop::Usage(format!("--pause-after wants bytes, not {text}")))?,
        ),
    };
    let cancel_after = match args.text("cancel-after") {
        None => None,
        Some(text) => Some(limit(&text).ok_or_else(|| {
            Stop::Usage(format!(
                "--cancel-after wants bytes or a count such as 3items, not {text}"
            ))
        })?),
    };
    Ok(Plan {
        conflict,
        on_error,
        pause_after,
        cancel_after,
    })
}

fn limit(text: &str) -> Option<Limit> {
    let items = text
        .strip_suffix("items")
        .or_else(|| text.strip_suffix("item"));
    match items {
        Some(count) => count.trim().parse().ok().map(Limit::Items),
        None => args::bytes(text).map(Limit::Bytes),
    }
}

fn verb(op: &Op) -> &'static str {
    match op {
        Op::Move { .. } => "Moving",
        Op::Copy { .. } | Op::CopyExactly { .. } | Op::Retry(_) => "Copying",
        Op::Duplicate(_) => "Duplicating",
        Op::Trash(_) => "Moving to the Trash",
        Op::DeleteImmediately(_) => "Deleting",
        Op::PutBack(_) => "Putting back",
        Op::MakeAlias(_) => "Making aliases",
        Op::Compress(_) => "Compressing",
        Op::Rename { .. } => "Renaming",
        Op::NewFolder { .. } | Op::NewFile { .. } => "Creating",
        Op::Quick(_) => "Working",
        Op::Batch(ops) => ops.first().map_or("Working", verb),
    }
}

fn perform(command: &str, op: Op, args: &Args, out: &mut Out) -> Result<u8, Stop> {
    let plan = plan(args, None)?;
    let journal = args.path("journal");
    args.done(command)?;
    let mut book = journal.as_deref().map(Journal::load).transpose()?;
    let ran = run::run(op.clone(), &plan, verb(&op), out);
    if let (Some(book), Some(path)) = (book.as_mut(), journal.as_deref()) {
        book.record(
            queue::label_for(&op),
            ran.done.undo.as_ref(),
            &op,
            plan.conflict,
            ran.left.as_ref().filter(|_| !ran.failures.is_empty()),
        );
        book.save(path)?;
    }
    ran.report(command, out);
    Ok(ran.code())
}

fn transfer(command: &str, mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let exactly = command == "copy" && args.flag("exactly");
    let mut paths = args.paths();
    let Some(into) = paths.pop().filter(|_| !paths.is_empty()) else {
        return Err(Stop::Usage(format!(
            "{command} needs a source and a folder"
        )));
    };
    if !list::is_dir(&into) {
        return Err(Stop::Refused(format!("{} is not a folder", into.display())));
    }
    let op = match (command, exactly) {
        ("move", _) => Op::Move { items: paths, into },
        (_, false) => Op::Copy { items: paths, into },
        (_, true) => Op::CopyExactly { items: paths, into },
    };
    perform(command, op, &args, out)
}

fn each(
    command: &str,
    mut args: Args,
    out: &mut Out,
    op: fn(Vec<PathBuf>) -> Op,
) -> Result<u8, Stop> {
    let items = args.paths();
    if items.is_empty() {
        return Err(Stop::Usage(format!("{command} needs an item")));
    }
    perform(command, op(items), &args, out)
}

/// The app asks before it clears a lock; here `--unlock` is the answer.
fn remove(command: &str, mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let unlock = args.flag("unlock");
    let items = args.paths();
    if items.is_empty() {
        return Err(Stop::Usage(format!("{command} needs an item")));
    }
    if let Some(held) = items
        .iter()
        .find(|item| ops::locked(item))
        .filter(|_| !unlock)
    {
        args.done(command)?;
        return Err(Stop::Refused(format!(
            "\u{201c}{}\u{201d} is locked; pass --unlock to {command} it anyway",
            held.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    let op = match command {
        "trash" => Op::Trash(items),
        _ => Op::DeleteImmediately(items),
    };
    perform(command, op, &args, out)
}

fn put_back(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let to = args.path("to");
    let items = args.paths();
    let back = match (to, items.as_slice()) {
        (_, []) => return Err(Stop::Usage("put-back needs an item in a Trash".into())),
        (Some(to), [item]) => vec![Restore {
            item: item.clone(),
            to,
        }],
        (Some(_), _) => return Err(Stop::Usage("--to names where one item goes".into())),
        (None, items) => {
            let origins = put_back::origins(items);
            let mut back = Vec::new();
            for (item, origin) in items.iter().zip(origins) {
                let Some(to) = origin else {
                    args.done("put-back")?;
                    return Err(Stop::Refused(format!(
                        "{} has no record of where it came from; pass --to",
                        item.display()
                    )));
                };
                back.push(Restore {
                    item: item.clone(),
                    to,
                });
            }
            back
        }
    };
    perform("put-back", Op::PutBack(back), &args, out)
}

fn rename(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let (Some(path), Some(name)) = (args.shift_path(), args.shift()) else {
        return Err(Stop::Usage("rename needs a path and a new name".into()));
    };
    if !args.rest().is_empty() {
        return Err(Stop::Usage("rename takes one path and one name".into()));
    }
    // Without `--conflict` a taken name is refused, as the app refuses it.
    if args.text("conflict").is_none() {
        let dest = path.with_file_name(&name);
        let taken = std::fs::symlink_metadata(&dest).is_ok();
        let same = |one: &std::path::Path, other: &std::path::Path| {
            use std::os::unix::fs::MetadataExt as _;
            let id = |at: &std::path::Path| {
                std::fs::symlink_metadata(at)
                    .ok()
                    .map(|meta| (meta.dev(), meta.ino()))
            };
            id(one).is_some() && id(one) == id(other)
        };
        if taken && !same(&path, &dest) && dest != path {
            args.done("rename")?;
            return Err(Stop::Refused(format!(
                "The name \u{201c}{name}\u{201d} is already taken. Please choose a different name."
            )));
        }
    }
    perform("rename", Op::Rename { path, name }, &args, out)
}

fn new_item(command: &str, mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let into = args
        .shift_path()
        .ok_or_else(|| Stop::Usage(format!("{command} needs a folder")))?;
    let name = args.shift();
    if !args.rest().is_empty() {
        return Err(Stop::Usage(format!("{command} takes a folder and a name")));
    }
    let op = match (command, name) {
        ("new-folder", name) => Op::NewFolder {
            into,
            name: name.unwrap_or_else(|| "untitled folder".into()),
        },
        (_, Some(name)) => Op::NewFile { into, name },
        (_, None) => return Err(Stop::Usage("new-file needs a name".into())),
    };
    perform(command, op, &args, out)
}

fn quick(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let action = args.shift().unwrap_or_default();
    let quick = match action.as_str() {
        "rotate-left" | "rotate-right" => Quick::Rotate {
            items: args.paths(),
            left: action == "rotate-left",
        },
        "pdf" => Quick::CreatePdf(args.paths()),
        "remove-background" => Quick::RemoveBackground(args.paths()),
        "convert" => {
            let format = args.text("format").unwrap_or_default();
            let format = Format::parse(&format)
                .ok_or_else(|| Stop::Usage("convert needs --format jpeg, png or heif".into()))?;
            let size = match args.text("size") {
                None => Size::Actual,
                Some(text) => Size::parse(&text).ok_or_else(|| {
                    Stop::Usage(format!(
                        "--size takes actual, large, medium or small, not {text}"
                    ))
                })?,
            };
            let step = Step::Convert {
                items: args.paths(),
                format,
                size,
                metadata: args.flag("metadata"),
            };
            match Op::from(step) {
                Op::Quick(quick) => quick,
                _ => unreachable!("a convert step is a quick action"),
            }
        }
        "trim" => {
            let item = args
                .shift_path()
                .ok_or_else(|| Stop::Usage("trim needs an item".into()))?;
            let (Some(start), Some(end)) = (args.number("start")?, args.number("end")?) else {
                return Err(Stop::Usage(
                    "trim needs --start and --end in seconds".into(),
                ));
            };
            Quick::Trim {
                into: args.path("into").unwrap_or_else(|| item.clone()),
                tags: finder_fs::tags::of(&item)
                    .into_iter()
                    .map(|tag| tag.name)
                    .collect(),
                item,
                start,
                end,
            }
        }
        _ => {
            return Err(Stop::Usage(
                "quick takes rotate-left, rotate-right, pdf, convert, remove-background or trim"
                    .into(),
            ));
        }
    };
    if quick.items().is_empty() {
        return Err(Stop::Usage(format!("quick {action} needs an item")));
    }
    perform("quick", Op::Quick(quick), &args, out)
}

/// The app's Undo, Redo and Retry: undo and redo keep both names, a retry merges.
fn replay(command: &str, mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let path = args
        .shift_path()
        .ok_or_else(|| Stop::Usage(format!("{command} needs a journal file")))?;
    let fixed = match command {
        "retry" => Conflict::Merge,
        _ => Conflict::KeepBoth,
    };
    let mut plan = plan(&args, Some(fixed))?;
    args.done(command)?;
    let mut book = Journal::load(&path)?;
    let nothing = |what: &str| Stop::Refused(format!("{} has nothing to {what}", path.display()));
    let ran = match command {
        "undo" => {
            let mut entry = book.pop_undo().ok_or_else(|| nothing("undo"))?;
            let ran = run::run(Op::from(entry.undo.clone()), &plan, "Undoing", out);
            match ran.left.as_ref().filter(|_| !ran.failures.is_empty()) {
                Some(left) => {
                    entry.undo = Step::from(left);
                    book.push_undo(entry);
                }
                None => book.push_redo(entry),
            }
            ran
        }
        "redo" => {
            let mut entry = book.pop_redo().ok_or_else(|| nothing("redo"))?;
            plan.conflict = entry.conflict.conflict();
            let ran = run::run(Op::from(entry.redo.clone()), &plan, "Redoing", out);
            if let Some(undo) = &ran.done.undo {
                entry.undo = Step::from(undo);
            }
            entry.left = left_of(&ran);
            book.push_undo(entry);
            ran
        }
        _ => {
            let entry = book.unfinished().ok_or_else(|| nothing("retry"))?;
            let left = entry
                .left
                .take()
                .map(Op::from)
                .expect("an unfinished entry");
            let ran = run::run(left, &plan, "Retrying", out);
            entry.fold(ran.done.undo.as_ref());
            entry.left = left_of(&ran);
            ran
        }
    };
    book.save(&path)?;
    ran.report(command, out);
    Ok(ran.code())
}

fn left_of(ran: &Ran) -> Option<Step> {
    match (ran.end, ran.failures.is_empty()) {
        (End::Cancelled, _) | (_, true) => None,
        _ => ran.left.as_ref().map(Step::from),
    }
}

fn show_journal(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let path = args
        .shift_path()
        .ok_or_else(|| Stop::Usage("journal needs a journal file".into()))?;
    args.done("journal")?;
    let book = Journal::load(&path)?;
    let (undo, redo) = book.stacks();
    if out.json {
        let labels = |entries: &[journal::Entry]| -> Vec<String> {
            entries
                .iter()
                .rev()
                .map(|entry| entry.label.clone())
                .collect()
        };
        out.emit(json!({ "event": "journal", "undo": labels(undo), "redo": labels(redo) }));
        return Ok(OK);
    }
    for entry in undo.iter().rev() {
        out.say(&format!("undo  {}", entry.label));
    }
    for entry in redo.iter().rev() {
        out.say(&format!("redo  {}", entry.label));
    }
    Ok(OK)
}

fn clean(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let folders = args.paths();
    args.done("clean")?;
    if folders.is_empty() {
        return Err(Stop::Usage("clean needs a folder".into()));
    }
    if let Some(other) = folders.iter().find(|folder| !list::is_dir(folder)) {
        return Err(Stop::Refused(format!(
            "{} is not a folder",
            other.display()
        )));
    }
    let mut cleaned = ops::Cleaned::default();
    for folder in &folders {
        let one = ops::clean_temps(folder);
        cleaned.removed.extend(one.removed);
        cleaned.trashed.extend(one.trashed);
        cleaned.failures.extend(one.failures);
    }
    let code = if cleaned.failures.is_empty() {
        OK
    } else {
        PARTIAL
    };
    if out.json {
        out.emit(json!({
            "event": "result",
            "command": "clean",
            "exit": code,
            "removed": cleaned.removed.iter().map(|path| out::path(path)).collect::<Vec<_>>(),
            "trashed": cleaned.trashed.iter().map(|restore| json!({
                "item": out::path(&restore.item),
                "from": out::path(&restore.to),
            })).collect::<Vec<_>>(),
            "failures": cleaned.failures.iter().map(|(path, error)| json!({
                "source": out::path(path),
                "error": error.to_string(),
            })).collect::<Vec<_>>(),
        }));
        return Ok(code);
    }
    for path in &cleaned.removed {
        out.say(&format!("removed {}", path.display()));
    }
    for restore in &cleaned.trashed {
        out.say(&format!(
            "trashed {} to {}",
            restore.to.display(),
            restore.item.display()
        ));
    }
    for (path, error) in &cleaned.failures {
        out.warn(&format!("{}: {error}", path.display()));
    }
    Ok(code)
}
