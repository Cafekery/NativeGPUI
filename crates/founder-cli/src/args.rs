use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::Stop;

/// Options that take a value, as `--name value` or `--name=value`.
const VALUED: &[&str] = &[
    "app",
    "cancel-after",
    "conflict",
    "end",
    "format",
    "home",
    "id",
    "index",
    "into",
    "journal",
    "limit",
    "on-error",
    "pause-after",
    "size",
    "sort",
    "start",
    "to",
];

const SHORT: &[(char, &str)] = &[('a', "all"), ('h', "help"), ('l', "long"), ('q', "quiet")];

pub struct Args {
    words: Vec<OsString>,
    flags: Vec<(String, Option<OsString>)>,
    used: RefCell<Vec<String>>,
}

impl Args {
    pub fn parse(raw: impl IntoIterator<Item = OsString>) -> Result<Args, Stop> {
        let mut words = Vec::new();
        let mut flags = Vec::new();
        let mut raw = raw.into_iter();
        while let Some(arg) = raw.next() {
            let text = arg.to_string_lossy();
            if text == "--" {
                words.extend(raw.by_ref());
                break;
            }
            if let Some(long) = text.strip_prefix("--") {
                let (name, value) = match long.split_once('=') {
                    Some((name, value)) => (name.to_string(), Some(OsString::from(value))),
                    None => (long.to_string(), None),
                };
                let value = match (VALUED.contains(&name.as_str()), value) {
                    (true, None) => Some(
                        raw.next()
                            .ok_or_else(|| Stop::Usage(format!("--{name} needs a value")))?,
                    ),
                    (false, Some(_)) => {
                        return Err(Stop::Usage(format!("--{name} takes no value")));
                    }
                    (_, value) => value,
                };
                flags.push((name, value));
                continue;
            }
            match text.strip_prefix('-').filter(|short| !short.is_empty()) {
                Some(short) => {
                    for letter in short.chars() {
                        let Some(&(_, name)) = SHORT.iter().find(|(c, _)| *c == letter) else {
                            return Err(Stop::Usage(format!("-{letter} is not an option")));
                        };
                        flags.push((name.to_string(), None));
                    }
                }
                None => words.push(arg),
            }
        }
        Ok(Args {
            words,
            flags,
            used: RefCell::new(Vec::new()),
        })
    }

    fn mark(&self, name: &str) {
        self.used.borrow_mut().push(name.to_string());
    }

    pub fn flag(&self, name: &str) -> bool {
        self.mark(name);
        self.flags.iter().any(|(flag, _)| flag == name)
    }

    pub fn value(&self, name: &str) -> Option<&OsStr> {
        self.mark(name);
        self.flags
            .iter()
            .rev()
            .find(|(flag, _)| flag == name)
            .and_then(|(_, value)| value.as_deref())
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.value(name)
            .map(|value| value.to_string_lossy().into_owned())
    }

    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.value(name).map(clean)
    }

    pub fn number(&self, name: &str) -> Result<Option<f64>, Stop> {
        self.text(name)
            .map(|text| {
                text.parse()
                    .map_err(|_| Stop::Usage(format!("--{name} wants a number, not {text}")))
            })
            .transpose()
    }

    pub fn shift(&mut self) -> Option<String> {
        (!self.words.is_empty()).then(|| self.words.remove(0).to_string_lossy().into_owned())
    }

    pub fn shift_path(&mut self) -> Option<PathBuf> {
        (!self.words.is_empty()).then(|| clean(&self.words.remove(0)))
    }

    pub fn rest(&mut self) -> Vec<OsString> {
        std::mem::take(&mut self.words)
    }

    pub fn paths(&mut self) -> Vec<PathBuf> {
        self.rest().iter().map(|word| clean(word)).collect()
    }

    /// Every option given was one the command read.
    pub fn done(&self, command: &str) -> Result<(), Stop> {
        let used = self.used.borrow();
        match self.flags.iter().find(|(flag, _)| !used.contains(flag)) {
            Some((flag, _)) => Err(Stop::Usage(format!(
                "--{flag} is not an option of `{command}`"
            ))),
            None => Ok(()),
        }
    }
}

/// Absolute, without `.` parts or a trailing slash: `link/` would name the folder a link points at.
pub fn clean(word: &OsStr) -> PathBuf {
    let path = Path::new(word);
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    absolute.components().collect()
}

pub fn bytes(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let scale: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        _ => return None,
    };
    number.checked_mul(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Args {
        Args::parse(words.iter().map(OsString::from)).unwrap_or_else(|_| panic!("{words:?}"))
    }

    #[test]
    fn options_take_values_either_way_and_words_keep_their_order() {
        let mut args = parse(&["copy", "--conflict", "merge", "a", "--json", "b", "--to=c"]);
        assert_eq!(args.shift().as_deref(), Some("copy"));
        assert_eq!(args.text("conflict").as_deref(), Some("merge"));
        assert_eq!(args.text("to").as_deref(), Some("c"));
        assert!(args.flag("json"));
        assert_eq!(args.rest(), [OsString::from("a"), OsString::from("b")]);
        assert!(args.done("copy").is_ok());
    }

    #[test]
    fn an_option_the_command_never_read_is_a_usage_error() {
        let args = parse(&["ls", "--exactly"]);
        assert!(!args.flag("all"));
        assert!(matches!(args.done("ls"), Err(Stop::Usage(_))));
    }

    #[test]
    fn a_double_dash_ends_the_options() {
        let mut args = parse(&["rename", "--", "--odd", "-x"]);
        args.shift();
        assert_eq!(args.rest(), [OsString::from("--odd"), OsString::from("-x")]);
    }

    #[test]
    fn a_trailing_slash_never_reaches_the_engine() {
        assert_eq!(clean(OsStr::new("/tmp/link/")), PathBuf::from("/tmp/link"));
        assert_eq!(clean(OsStr::new("/tmp/./a")), PathBuf::from("/tmp/a"));
        assert!(clean(OsStr::new("relative")).is_absolute());
    }

    #[test]
    fn sizes_read_binary_units() {
        assert_eq!(bytes("512"), Some(512));
        assert_eq!(bytes("4k"), Some(4096));
        assert_eq!(bytes("3M"), Some(3 << 20));
        assert_eq!(bytes("2GB"), Some(2 << 30));
        assert_eq!(bytes("2 items"), None);
        assert_eq!(bytes("x"), None);
    }
}
