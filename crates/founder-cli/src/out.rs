use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::time::Duration;

use finder_fs::ops::transfer::{Phase, Progress};
use serde_json::{Value, json};

/// JSON lines on stdout with `--json`; otherwise text, and a progress line when stderr is a terminal.
pub struct Out {
    pub json: bool,
    live: bool,
    drawn: bool,
}

impl Out {
    pub fn new(json: bool, quiet: bool) -> Out {
        Out {
            json,
            live: !json && !quiet && std::io::stderr().is_terminal(),
            drawn: false,
        }
    }

    pub fn emit(&mut self, value: Value) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{value}");
        let _ = stdout.flush();
    }

    pub fn say(&mut self, text: &str) {
        self.clear();
        println!("{text}");
    }

    pub fn warn(&mut self, text: &str) {
        self.clear();
        eprintln!("founder: {text}");
    }

    pub fn progress(&mut self, verb: &str, progress: &Progress, elapsed: Duration) {
        if self.json {
            return self.emit(progress_json(progress, elapsed));
        }
        if !self.live {
            return;
        }
        let line = progress_line(verb, progress);
        let width = terminal_width();
        let line: String = line.chars().take(width.saturating_sub(1)).collect();
        eprint!("\r\x1b[K{line}");
        let _ = std::io::stderr().flush();
        self.drawn = true;
    }

    pub fn clear(&mut self) {
        if self.drawn {
            eprint!("\r\x1b[K");
            self.drawn = false;
        }
    }
}

fn terminal_width() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    match unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) } {
        0 if size.ws_col > 0 => usize::from(size.ws_col),
        _ => 100,
    }
}

pub fn path(path: &Path) -> Value {
    Value::String(path.to_string_lossy().into_owned())
}

fn phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Copying => "copying",
        Phase::Finishing => "finishing",
        Phase::Done => "done",
    }
}

fn progress_json(progress: &Progress, elapsed: Duration) -> Value {
    json!({
        "event": "progress",
        "secs": (elapsed.as_secs_f64() * 1000.).round() / 1000.,
        "phase": phase(progress.phase),
        "counting": progress.counting,
        "paused": progress.paused,
        "bytes_done": progress.bytes_done,
        "bytes_total": progress.bytes_total,
        "items_done": progress.items_done,
        "items_total": progress.items_total,
        "failures": progress.failures,
        "bytes_per_sec": progress.bytes_per_sec.round(),
        "eta_secs": progress.eta.map(|eta| eta.as_secs_f64()),
        "current": progress.current.as_ref().map(|current| json!({
            "path": path(&current.path),
            "bytes_done": current.bytes_done,
            "bytes_total": current.bytes_total,
        })),
    })
}

fn progress_line(verb: &str, progress: &Progress) -> String {
    let mut line = match progress.counting {
        true => format!("{verb} {} items so far", progress.items_total),
        false => format!(
            "{verb} {} of {}",
            progress.items_done.min(progress.items_total),
            progress.items_total
        ),
    };
    if progress.bytes_total > 0 {
        line += &format!(
            ", {} of {}",
            size(progress.bytes_done),
            size(progress.bytes_total)
        );
    }
    if progress.paused {
        line += ", paused";
    } else if progress.bytes_per_sec > 0. {
        line += &format!(", {}/s", size(progress.bytes_per_sec as u64));
        if let Some(eta) = progress.eta {
            line += &format!(", {}", time_left(eta));
        }
    }
    if let Some(current) = &progress.current {
        let name = current.path.file_name().unwrap_or_default();
        line += &format!(
            " \u{2014} {}",
            finder_fs::name::display(&name.to_string_lossy())
        );
    }
    line
}

/// Finder's units: powers of 1000.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return match bytes {
            1 => "1 byte".into(),
            n => format!("{n} bytes"),
        };
    }
    let mut value = bytes as f64 / 1000.;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.;
        unit += 1;
    }
    match unit {
        0 => format!("{value:.0} {}", UNITS[unit]),
        1 => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.2} {}", UNITS[unit]),
    }
}

fn time_left(eta: Duration) -> String {
    match eta.as_secs() {
        0..5 => "a few seconds left".into(),
        secs @ 5..60 => format!("about {secs} s left"),
        secs @ 60..3600 => format!("about {} min left", secs.div_ceil(60)),
        secs => format!("about {:.1} h left", secs as f64 / 3600.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1000), "1 KB");
        assert_eq!(size(12_345_678), "12.3 MB");
        assert_eq!(size(4_294_967_296), "4.29 GB");
    }
}
