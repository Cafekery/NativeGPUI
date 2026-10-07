//! A search as Finder's field holds it: tokens, each matching one attribute,
//! then the text typed after them. A result matches all of them.

use std::time::{SystemTime, UNIX_EPOCH};

use finder_fs::sort::fold;

use crate::data::Row;
use crate::listing::folds_to;
use crate::smart::escape;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Term {
    Name(String),
    Everything(String),
    /// A content type, matched with everything that conforms to it.
    Kind(String),
    Tag(String),
    /// Seconds since 1970, the end excluded.
    Date(f64, f64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Search {
    pub(crate) terms: Vec<Term>,
    pub(crate) text: String,
}

/// Umbrella kinds declare no extensions of their own; fsearch narrows to them by group.
const GROUPS: [(&str, &str); 11] = [
    ("public.folder", "kind:dir"),
    ("com.apple.application", "type:app"),
    ("public.image", "type:image"),
    ("public.movie", "type:video"),
    ("public.audiovisual-content", "type:video type:audio"),
    ("public.audio", "type:audio"),
    ("public.archive", "type:archive"),
    ("public.font", "type:font"),
    ("public.source-code", "type:code"),
    ("public.plain-text", "type:code ext:txt,text,md,markdown"),
    ("public.text", "type:code ext:txt,text,md,markdown,rtf,html"),
];

fn push(query: &mut String, part: &str) {
    if part.is_empty() {
        return;
    }
    if !query.is_empty() {
        query.push(' ');
    }
    query.push_str(part);
}

/// One run in fsearch's language, spaces and all.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.trim().replace('"', ""))
}

fn exact(text: &str) -> String {
    quoted(&format!("'{}", text.trim()))
}

/// fsearch's own syntax, which only names answer to.
fn syntax(word: &str) -> bool {
    word.contains(['"', '/'])
        || word.starts_with(['!', '\'', '^'])
        || word.ends_with('$')
        || word
            .split_once(':')
            .is_some_and(|(key, _)| ::fsearch::query::is_filter(key))
}

fn kind_filter(uti: &str) -> String {
    if let Some((_, filter)) = GROUPS.iter().find(|(group, _)| *group == uti) {
        return (*filter).to_owned();
    }
    let exts = finder_fs::kind::extensions(uti);
    match exts.is_empty() {
        true => String::new(),
        false => format!("ext:{}", exts.join(",")),
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0., |since| since.as_secs_f64())
}

/// fsearch reads a modification date as an age: `mtime:1s..9s` is between one and nine seconds old.
fn date_filter(from: f64, to: f64) -> String {
    let now = now();
    if from > now {
        return String::new();
    }
    let (young, old) = ((now - to).max(0.), now - from);
    format!("mtime:{}s..{}s", young.floor(), old.ceil())
}

impl Search {
    pub(crate) fn typed(text: String) -> Self {
        Self {
            terms: Vec::new(),
            text,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.text.trim().is_empty()
    }

    fn plain(&self) -> bool {
        !self.text.split_whitespace().any(syntax)
    }

    /// The kind and date filters, in fsearch's language.
    pub(crate) fn filters(&self) -> String {
        let mut filters = String::new();
        for term in &self.terms {
            match term {
                Term::Kind(uti) => push(&mut filters, &kind_filter(uti)),
                Term::Date(from, to) => push(&mut filters, &date_filter(*from, *to)),
                Term::Name(_) | Term::Everything(_) | Term::Tag(_) => {}
            }
        }
        filters
    }

    /// fsearch's query for names: the typed text in its own language, each
    /// name or everything token as one exact run, and the filters.
    pub(crate) fn names(&self) -> String {
        let mut query = self.text.trim().to_owned();
        for term in &self.terms {
            if let Term::Name(text) | Term::Everything(text) = term {
                push(&mut query, &exact(text));
            }
        }
        push(&mut query, &self.filters());
        query
    }

    /// What Finder also looks for inside files: each everything token and
    /// each plain word typed. Typed text in fsearch's syntax searches names.
    pub(crate) fn contents(&self) -> Vec<String> {
        let typed = self.plain().then(|| self.text.split_whitespace());
        self.terms
            .iter()
            .filter_map(|term| match term {
                Term::Everything(text) => Some(text.trim()),
                _ => None,
            })
            .chain(typed.into_iter().flatten())
            .filter(|text| text.chars().nth(1).is_some())
            .map(str::to_owned)
            .collect()
    }

    /// fsearch's query for the files whose content holds `word`: every name
    /// token and any typed syntax narrow which files are read.
    pub(crate) fn content(&self, word: &str) -> String {
        let mut query = format!("grep:{}", quoted(word));
        if !self.plain() {
            push(&mut query, self.text.trim());
        }
        for term in &self.terms {
            if let Term::Name(text) = term {
                push(&mut query, &exact(text));
            }
        }
        push(&mut query, &self.filters());
        query
    }

    /// Spotlight's query for everything that mentions `word`, as Finder's own
    /// Contains asks it, narrowed to the kinds.
    pub(crate) fn spotlight(&self, word: &str) -> Option<String> {
        if !self.plain() {
            return None;
        }
        let mut query = format!("(** == \"{}*\"cdw)", escape(word));
        for term in &self.terms {
            if let Term::Kind(uti) = term {
                query.push_str(&format!(
                    " && (kMDItemContentTypeTree == \"{}\")",
                    escape(uti)
                ));
            }
        }
        Some(query)
    }

    /// Spotlight's query for the tags, which only it knows.
    pub(crate) fn tagged(&self) -> Option<String> {
        let tags: Vec<String> = self
            .terms
            .iter()
            .filter_map(|term| match term {
                Term::Tag(tag) => Some(format!("(kMDItemUserTags == \"{}\"c)", escape(tag))),
                _ => None,
            })
            .collect();
        (!tags.is_empty()).then(|| tags.join(" && "))
    }

    pub(crate) fn matcher(&self) -> Matcher {
        let typed = self.plain().then(|| self.text.split_whitespace());
        let mut matcher = Matcher {
            typed: fold(self.text.trim()),
            names: Vec::new(),
            everything: typed.into_iter().flatten().map(fold).collect(),
            kinds: Vec::new(),
            dates: Vec::new(),
            tagged: false,
        };
        for term in &self.terms {
            match term {
                Term::Name(text) => matcher.names.push(fold(text.trim())),
                Term::Everything(text) => matcher.everything.push(fold(text.trim())),
                Term::Kind(uti) => matcher.kinds.push(uti.clone()),
                Term::Date(from, to) => matcher.dates.push((*from, *to)),
                Term::Tag(_) => matcher.tagged = true,
            }
        }
        matcher
    }
}

pub(crate) struct Matcher {
    typed: String,
    names: Vec<String>,
    everything: Vec<String>,
    kinds: Vec<String>,
    dates: Vec<(f64, f64)>,
    tagged: bool,
}

impl Matcher {
    /// A row of the folder on screen, which matches by its name, kind and
    /// dates alone. Tags are Spotlight's to answer.
    pub(crate) fn admits(&self, row: &Row) -> bool {
        !self.tagged
            && (self.typed.is_empty() || folds_to(&row.name, &self.typed))
            && self.names.iter().all(|name| folds_to(&row.name, name))
            && self.everything.iter().all(|word| folds_to(&row.name, word))
            && self.fits(row)
    }

    /// What fsearch can only narrow to: the kinds and the dates.
    pub(crate) fn fits(&self, row: &Row) -> bool {
        self.kinds
            .iter()
            .all(|uti| finder_fs::kind::conforms_to(&row.uti, uti))
            && self.dates.iter().all(|&(from, to)| {
                [Some(row.modified_at), Some(row.created_at), row.added_at]
                    .into_iter()
                    .flatten()
                    .any(|at| at >= from && at < to)
            })
    }

    /// A row found by what it holds or by its tags: every name token holds
    /// by name, every everything word by name or by `held(word)`.
    pub(crate) fn admits_found(&self, row: &Row, held: impl Fn(&str) -> bool) -> bool {
        self.names.iter().all(|name| folds_to(&row.name, name))
            && self
                .everything
                .iter()
                .all(|word| folds_to(&row.name, word) || held(word))
            && self.fits(row)
    }
}
