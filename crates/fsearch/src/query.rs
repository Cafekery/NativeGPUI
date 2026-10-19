//! Name search: parse a query, scan the index in parallel, rank, top-k.

use crate::index::{BM_DOUBLE, BM_FIRST, BM_SECOND, BM_START, CLASSES, Index, ZONE, Zone, char_bit, start_bit};
use crate::live::Live;
use crate::walk::{FLAG_HIDDEN, KIND_DIR, KIND_FILE, KIND_LINK};
use rayon::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Fuzzy,
    Exact,
    Prefix,
    Suffix,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub text: Vec<u8>,
    pub mask: u64,
    pub mode: Mode,
    pub negate: bool,
    /// Char classes a typo may leave out of a matching name.
    pub loose: u64,
    pub start: u64,
}

impl Token {
    #[inline(always)]
    fn fits(&self, m: u64) -> bool {
        let miss = self.mask & !m;
        (miss == 0) | ((((miss & !self.loose) | (miss & miss.wrapping_sub(1))) == 0) & (m & self.start != 0))
    }

    fn literal(text: Vec<u8>) -> Token {
        let mask = text.iter().fold(0, |m, &b| m | char_bit(b));
        Token { text, mask, mode: Mode::Exact, negate: false, loose: 0, start: 0 }
    }

    #[inline(always)]
    fn known_mask(&self, clean: bool, start: bool, spaced: bool) -> u64 {
        (if clean { self.mask } else { 0 }) | (if start { self.start } else { 0 }) | (if spaced { char_bit(b' ') } else { 0 })
    }
}

struct Cand {
    bound: i32,
    k: u32,
    score: i16,
    flags: u8,
}

#[derive(Clone, Copy)]
#[repr(align(128))]
struct Line<T>(T);

type Pair = Line<[(u64, u64, u64); 2]>;

struct TokenBits<'a> {
    seq: Vec<[&'a [u64]; 2]>,
    classes: Vec<[&'a [u64]; 2]>,
    start: Option<&'a [u64]>,
}

impl<'a> TokenBits<'a> {
    fn new(idx: &'a Index, t: &Token) -> TokenBits<'a> {
        let class = |b: u8| char_bit(b).trailing_zeros() as usize;
        let pair = |c: usize| [idx.bitmap(BM_FIRST + c), idx.bitmap(BM_SECOND + c)];
        let first = class(t.text[0]);
        let (counts, doubles) = (idx.class_counts(), idx.double_counts());
        let twice = crate::index::doubled(&t.text);
        let mut rest: Vec<(u32, [&[u64]; 2])> =
            (0..CLASSES).filter(|&c| c != first && t.mask & (1 << c) != 0).map(|c| (counts[c], pair(c))).collect();
        rest.extend((0..CLASSES).filter(|&c| twice & (1 << c) != 0).map(|c| (doubles[c], [idx.bitmap(BM_DOUBLE + c); 2])));
        rest.sort_by_key(|x| x.0);
        TokenBits {
            seq: t.text.iter().map(|&b| pair(class(b))).collect(),
            classes: std::iter::once(pair(first)).chain(rest.into_iter().map(|x| x.1)).collect(),
            start: (t.start != 0).then(|| idx.bitmap(BM_START + crate::index::start_hash(t.text[0]))),
        }
    }

    #[inline]
    fn word(&self, w: usize) -> (u64, u64, u64) {
        self.words::<1>(w)[0]
    }

    #[inline(always)]
    fn words<const B: usize>(&self, w: usize) -> [(u64, u64, u64); B] {
        let has = |b: &[&[u64]; 2], j: usize| b[0][w + j] | b[1][w + j];
        let none = |x: &[[u64; B]]| x.iter().flatten().fold(0, |a, &b| a | b) == 0;
        let mut all: [u64; B] = std::array::from_fn(|j| has(&self.classes[0], j));
        let typo: [u64; B] = self.start.map_or([0; B], |s| std::array::from_fn(|j| s[w + j] & all[j]));
        let mut one = [0; B];
        for b in &self.classes[1..] {
            if none(&[all, one]) {
                break;
            }
            for j in 0..B {
                let x = has(b, j);
                one[j] = (one[j] & x) | (all[j] & !x & typo[j]);
                all[j] &= x;
            }
        }
        let (mut first, mut second): ([u64; B], _) = (std::array::from_fn(|j| all[j] | one[j]), [0; B]);
        let (mut first1, mut second1) = ([0; B], [0; B]);
        for (k, b) in self.seq.iter().enumerate() {
            if none(&[first, second, first1, second1]) {
                break;
            }
            let skip = if k > 0 { !0 } else { 0 };
            for j in 0..B {
                let (b0, b1) = (b[0][w + j], b[1][w + j]);
                second1[j] = ((second1[j] | first1[j]) & b1) | (second[j] & typo[j] & skip);
                first1[j] = (first1[j] & b0) | (first[j] & typo[j] & skip);
                second[j] = (second[j] | first[j]) & b1;
                first[j] &= b0;
            }
        }
        std::array::from_fn(|j| {
            let clean = (first[j] | second[j]) & all[j];
            let typo = (first[j] | second[j] | first1[j] | second1[j]) & typo[j];
            (clean | typo, clean, typo)
        })
    }
}

fn name_hit(idx: &Index, q: &Query, pos: &[&Token], neg: &[&Token], re: Option<&regex::bytes::Regex>, k: u32) -> Option<NameHit> {
    let name = idx.uname(k);
    let m = crate::index::name_mask(name);
    let mut h = NameHit { score: 0, bits: 0, flags: name_flags(name), best: [0; 4] };
    for (t, tok) in pos.iter().enumerate() {
        if tok.fits(m)
            && let Some(s) = token_score(name, idx.uname_wide(k), m, tok)
        {
            h.bits |= 1 << t;
            let s16 = s.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            h.score = h.score.saturating_add(s16);
            if t < 4 {
                h.best[t] = s16.max(0);
            }
        }
    }
    if neg.iter().any(|t| token_matches(name, t)) {
        h.flags |= NF_NEG;
    }
    let ok = (pos.is_empty() || h.bits != 0)
        && h.flags & NF_NEG == 0
        && (q.exts.is_empty() || ext_ok(name, &q.exts))
        && re.is_none_or(|re| re.is_match(name));
    if ok {
        h.flags |= NF_OK;
    }
    (ok || h.bits != 0 || h.flags & NF_NEG != 0).then_some(h)
}

fn space_bits(idx: &Index) -> [&[u64]; 2] {
    let c = char_bit(b' ').trailing_zeros() as usize;
    [idx.bitmap(BM_FIRST + c), idx.bitmap(BM_SECOND + c)]
}

fn re_literals(re: &regex::bytes::Regex) -> Option<Vec<Vec<u8>>> {
    let hir = regex_syntax::ParserBuilder::new().utf8(false).build().parse(re.as_str()).ok()?;
    let mut lits: Vec<Vec<u8>> =
        regex_syntax::hir::literal::Extractor::new().extract(&hir).literals()?.iter().map(|l| l.as_bytes().to_ascii_lowercase()).collect();
    lits.sort();
    lits.dedup();
    lits.iter().all(|l| !l.is_empty()).then_some(lits)
}

fn path_suffixes(re: &regex::bytes::Regex) -> Option<Vec<Vec<u8>>> {
    use regex_syntax::hir::{Look, literal::ExtractKind};
    let hir = regex_syntax::ParserBuilder::new().utf8(false).build().parse(re.as_str()).ok()?;
    if !hir.properties().look_set_suffix().contains(Look::End) {
        return None;
    }
    let seq = regex_syntax::hir::literal::Extractor::new().kind(ExtractKind::Suffix).extract(&hir);
    let mut lits: Vec<Vec<u8>> = seq.literals()?.iter().map(|l| l.as_bytes().to_ascii_lowercase()).collect();
    lits.sort();
    lits.dedup();
    lits.iter().all(|l| !l.is_empty() && !l.contains(&b'/')).then_some(lits)
}

fn ends_with_fold(name: &[u8], s: &[u8]) -> bool {
    name.len() >= s.len() && name[name.len() - s.len()..].iter().zip(s).all(|(&a, &b)| fold(a) == b)
}

const TYPO_MIN_LEN: usize = 5;
const TYPO_COST: i32 = 60;

fn takes_typos(text: &[u8], mode: Mode) -> bool {
    mode == Mode::Fuzzy && text.len() >= TYPO_MIN_LEN
}

#[derive(Clone, Default)]
pub struct Query {
    pub tokens: Vec<Token>,
    pub kind: Option<u8>,
    /// `type:app` also takes symlinks: the system apps in /Applications link into the cryptex.
    pub apps: bool,
    pub exts: Vec<Vec<u8>>,
    pub scope: Option<Vec<u8>>,
    pub size: (u64, u64),
    pub mtime: (u32, u32),
    pub name_re: Option<regex::bytes::Regex>,
    pub path_re: Option<regex::bytes::Regex>,
    pub limit: usize,
    pub grep: Option<String>,
    pub grep_mode: GrepMode,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum GrepMode {
    #[default]
    Literal,
    Regex,
    Symbol,
}

pub struct Hit {
    pub score: i32,
    /// u32::MAX for an overlay entry, whose path is then `over`.
    pub idx: u32,
    pub over: Option<Vec<u8>>,
}

#[rustfmt::skip]
const TYPES: &[(&str, &[&str])] = &[
    ("image", &["png", "jpg", "jpeg", "gif", "heic", "heif", "webp", "tiff", "tif", "bmp", "svg", "raw", "cr2", "cr3", "nef", "arw", "dng", "psd", "ico", "icns", "avif", "jxl"]),
    ("video", &["mp4", "mov", "m4v", "mkv", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "3gp", "hevc"]),
    ("audio", &["mp3", "m4a", "aac", "wav", "flac", "aiff", "aif", "ogg", "opus", "alac", "caf", "mid", "midi", "m4r"]),
    ("doc", &["pdf", "doc", "docx", "pages", "txt", "md", "rtf", "odt", "key", "ppt", "pptx", "numbers", "xls", "xlsx", "csv", "epub", "tex"]),
    ("code", &["rs", "c", "h", "cc", "cpp", "hpp", "m", "mm", "swift", "go", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "java", "kt", "rb", "php", "cs", "sh", "zsh", "bash", "fish", "lua", "sql", "html", "css", "scss", "json", "yaml", "yml", "toml", "xml", "vue", "svelte", "zig", "nim", "hs", "ml", "ex", "exs", "erl", "clj", "dart", "r", "jl", "metal", "glsl", "wgsl", "proto", "graphql", "nix"]),
    ("archive", &["zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "dmg", "pkg", "iso", "zst", "lz4", "xip"]),
    ("font", &["ttf", "otf", "woff", "woff2", "ttc", "dfont"]),
];

impl Query {
    /// Parse the query language: fuzzy words, `'x` exact, `^x` prefix, `x$` suffix, `!x` negate, and `ext: type: kind: in: size: mtime: re: path: limit: grep: regex: sym:` filters.
    pub fn parse(s: &str, home: &str) -> Result<Query, String> {
        let mut q = Query { size: (0, u64::MAX), mtime: (0, u32::MAX), limit: 50, ..Default::default() };
        for word in split_words(s) {
            if let Some((k, v)) = word.split_once(':')
                && q.filter(k, v, home)?
            {
                continue;
            }
            for piece in word.split('/').filter(|p| !p.is_empty()) {
                q.push_token(piece);
            }
        }
        Ok(q)
    }

    pub fn push_token(&mut self, w: &str) {
        let (mut t, mut negate, mut mode) = (w, false, Mode::Fuzzy);
        if let Some(r) = t.strip_prefix('!') {
            (t, negate, mode) = (r, true, Mode::Exact);
        }
        if let Some(r) = t.strip_prefix('\'') {
            (t, mode) = (r, Mode::Exact);
        } else if let Some(r) = t.strip_prefix('^') {
            (t, mode) = (r, Mode::Prefix);
        } else if let Some(r) = t.strip_suffix('$') {
            (t, mode) = (r, Mode::Suffix);
        }
        if t.is_empty() || (!negate && self.tokens.iter().filter(|t| !t.negate).count() >= 8) {
            return;
        }
        let text: Vec<u8> = t.bytes().map(|b| b.to_ascii_lowercase()).collect();
        let mask = text.iter().fold(0, |m, &b| m | char_bit(b));
        let (loose, start) = if takes_typos(&text, mode) { (mask & !char_bit(text[0]), start_bit(text[0])) } else { (0, 0) };
        self.tokens.push(Token { text, mask, mode, negate, loose, start });
    }

    pub fn filter(&mut self, k: &str, v: &str, home: &str) -> Result<bool, String> {
        match k {
            "ext" => self.exts.extend(v.split(',').map(|e| e.trim_start_matches('.').to_ascii_lowercase().into_bytes())),
            "type" => {
                for t in v.split(',') {
                    if t == "app" {
                        self.kind = Some(KIND_DIR);
                        self.apps = true;
                        self.exts.push(b"app".to_vec());
                        continue;
                    }
                    let (_, exts) = TYPES.iter().find(|(n, _)| *n == t).ok_or(format!("unknown type {t}"))?;
                    self.exts.extend(exts.iter().map(|e| e.as_bytes().to_vec()));
                }
            }
            "kind" => {
                self.kind = Some(match v {
                    "file" | "f" => KIND_FILE,
                    "dir" | "folder" | "d" => KIND_DIR,
                    "link" | "symlink" | "l" => KIND_LINK,
                    _ => return Err(format!("unknown kind {v}")),
                })
            }
            "in" => {
                let p = v.strip_prefix('~').map_or(v.to_string(), |r| format!("{home}{r}"));
                self.scope = Some(real_path(p).trim_end_matches('/').as_bytes().to_vec());
            }
            "size" => self.size = range(v, parse_size)?,
            "mtime" | "modified" => {
                // mtime:<7d means "modified within the last 7 days".
                let now = now_secs();
                let (lo, hi) = range(v, parse_age)?;
                self.mtime = (now.saturating_sub(hi.min(now as u64) as u32), now.saturating_sub(lo as u32));
                if hi == u64::MAX {
                    self.mtime.0 = 0;
                }
            }
            "re" => self.name_re = Some(regex::bytes::Regex::new(&format!("(?i){v}")).map_err(|e| e.to_string())?),
            "path" => self.path_re = Some(regex::bytes::Regex::new(&format!("(?i){v}")).map_err(|e| e.to_string())?),
            "limit" => self.limit = v.parse().map_err(|_| "bad limit")?,
            "grep" | "content" => (self.grep, self.grep_mode) = (Some(v.to_string()), GrepMode::Literal),
            "regex" => (self.grep, self.grep_mode) = (Some(v.to_string()), GrepMode::Regex),
            "sym" | "symbol" => (self.grep, self.grep_mode) = (Some(v.to_string()), GrepMode::Symbol),
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn clone_for_scan(&self) -> Query {
        Query { grep: None, ..self.clone() }
    }

    #[inline(always)]
    pub fn kind_ok(&self, kind: u8) -> bool {
        match self.kind {
            None => true,
            Some(want) => kind & 3 == want || (self.apps && kind & 3 == KIND_LINK),
        }
    }

    pub fn match_path(&self, path: &[u8], kind: u8, size: u64, mtime: u32) -> Option<i32> {
        self.match_path_with(path, kind, size, mtime, |dirs| self.dir_match(dirs))
    }

    pub fn match_path_with(&self, path: &[u8], kind: u8, size: u64, mtime: u32, dirs: impl FnOnce(&[u8]) -> DirMatch) -> Option<i32> {
        if let Some(s) = &self.scope
            && !(path.starts_with(s) && path.get(s.len()) == Some(&b'/'))
        {
            return None;
        }
        let cut = path.iter().rposition(|&b| b == b'/').unwrap_or(0);
        let name = &path[cut + 1..];
        if name.is_empty()
            || !self.kind_ok(kind)
            || (!self.exts.is_empty() && !ext_ok(name, &self.exts))
            || size < self.size.0
            || size > self.size.1
            || mtime < self.mtime.0
            || mtime > self.mtime.1
        {
            return None;
        }
        if self.tokens.is_empty() {
            let re_ok = self.name_re.as_ref().is_none_or(|re| re.is_match(name)) && self.path_re.as_ref().is_none_or(|re| re.is_match(path));
            return re_ok.then_some(0);
        }
        let mut pos = self.tokens.iter().filter(|t| !t.negate).peekable();
        let m = crate::index::name_mask(name);
        if pos.peek().is_some() && !pos.any(|t| t.fits(m) && token_score(name, None, m, t).is_some()) {
            return None;
        }
        if self.tokens.iter().any(|t| t.negate && token_matches(name, t)) {
            return None;
        }
        let d = dirs(&path[..cut]);
        if d.negated {
            return None;
        }
        let pos: Vec<&Token> = self.tokens.iter().filter(|t| !t.negate).collect();
        let all = (1u32 << pos.len()) - 1;
        let (mut got, mut inherited, mut score) = (0u32, 0u32, 0i32);
        for (t, tok) in pos.iter().enumerate() {
            if let Some(s) = token_score(name, None, m, tok) {
                got |= 1 << t;
                score += s;
            } else if let Some(s) = d.best[t] {
                inherited |= 1 << t;
                score += s * 3 / 4;
            }
        }
        if !pos.is_empty() && (got == 0 || (got | inherited) != all) {
            return None;
        }
        if self.name_re.as_ref().is_some_and(|re| !re.is_match(name)) || self.path_re.as_ref().is_some_and(|re| !re.is_match(path)) {
            return None;
        }
        Some(score)
    }

    pub fn dir_match(&self, dirs: &[u8]) -> DirMatch {
        let comps = || dirs.split(|&b| b == b'/').filter(|c| !c.is_empty());
        let mut d = DirMatch { negated: false, best: [None; 8] };
        d.negated = self.tokens.iter().any(|t| t.negate && comps().any(|c| token_matches(c, t)));
        for (t, tok) in self.tokens.iter().filter(|t| !t.negate).enumerate() {
            d.best[t] = comps().filter_map(|c| token_score(c, None, !0, tok)).max();
        }
        d
    }
}

#[derive(Clone, Copy)]
pub struct DirMatch {
    negated: bool,
    best: [Option<i32>; 8],
}

/// The index holds real paths (/etc is /private/etc).
fn real_path(p: String) -> String {
    static LAST: std::sync::Mutex<Option<(String, String, std::time::Instant)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if let Some((from, to, at)) = &*last
        && *from == p
        && at.elapsed() < std::time::Duration::from_secs(1)
    {
        return to.clone();
    }
    let to = std::fs::canonicalize(&p).map_or_else(|_| p.clone(), |c| c.to_string_lossy().into_owned());
    *last = Some((p, to.clone(), std::time::Instant::now()));
    to
}

fn split_words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn range(v: &str, p: fn(&str) -> Option<u64>) -> Result<(u64, u64), String> {
    let bad = || format!("bad range {v}");
    if let Some(r) = v.strip_prefix(">=").or(v.strip_prefix('>')) {
        return Ok((p(r).ok_or_else(bad)?, u64::MAX));
    }
    if let Some(r) = v.strip_prefix("<=").or(v.strip_prefix('<')) {
        return Ok((0, p(r).ok_or_else(bad)?));
    }
    if let Some((a, b)) = v.split_once("..") {
        return Ok((p(a).ok_or_else(bad)?, p(b).ok_or_else(bad)?));
    }
    let x = p(v).ok_or_else(bad)?;
    Ok((x, x))
}

fn split_unit(s: &str) -> (f64, String) {
    let i = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    (s[..i].parse().unwrap_or(f64::NAN), s[i..].to_ascii_lowercase())
}

fn parse_size(s: &str) -> Option<u64> {
    let (n, u) = split_unit(s);
    let m = match u.as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        _ => return None,
    };
    (!n.is_nan()).then_some((n * m) as u64)
}

fn parse_age(s: &str) -> Option<u64> {
    let (n, u) = split_unit(s);
    let m = match u.as_str() {
        "s" => 1.0,
        "m" | "min" => 60.0,
        "h" => 3600.0,
        "" | "d" => 86400.0,
        "w" => 604800.0,
        "mo" => 2592000.0,
        "y" => 31536000.0,
        _ => return None,
    };
    (!n.is_nan()).then_some((n * m) as u64)
}

pub fn now_secs() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32)
}

#[inline(always)]
pub(crate) fn fold(b: u8) -> u8 {
    b | (((b.wrapping_sub(b'A') < 26) as u8) << 5)
}

#[inline(always)]
fn is_subseq(name: &[u8], q: &[u8]) -> bool {
    let mut j = 0;
    for &b in name {
        if fold(b) == q[j] {
            j += 1;
            if j == q.len() {
                return true;
            }
        }
    }
    false
}

fn find_ci(name: &[u8], q: &[u8]) -> Option<usize> {
    if q.len() > name.len() {
        return None;
    }
    (0..=name.len() - q.len()).find(|&i| name[i..i + q.len()].iter().zip(q).all(|(&a, &b)| fold(a) == b))
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Lower,
    Upper,
    Digit,
    Delim,
    Other,
}

#[inline(always)]
fn class(b: u8) -> Class {
    match b {
        b'a'..=b'z' => Class::Lower,
        b'A'..=b'Z' => Class::Upper,
        b'0'..=b'9' => Class::Digit,
        b' ' | b'_' | b'-' | b'.' | b'/' | b'(' | b')' | b'[' | b']' | b',' | b'+' | b'@' => Class::Delim,
        _ => Class::Other,
    }
}

const SCORE_MATCH: i32 = 16;
const GAP_START: i32 = -3;
const GAP_EXT: i32 = -1;
const BONUS_BOUNDARY: i32 = 8;
const BONUS_CAMEL: i32 = 7;
const BONUS_CONSEC: i32 = 4;

#[inline(always)]
fn bonus(prev: Class, cur: Class) -> i32 {
    match (prev, cur) {
        (Class::Delim, c) if c != Class::Delim => BONUS_BOUNDARY,
        (Class::Lower, Class::Upper) | (Class::Lower | Class::Upper, Class::Digit) => BONUS_CAMEL,
        _ => 0,
    }
}

pub fn fuzzy_score(name: &[u8], q: &[u8]) -> Option<i32> {
    fuzzy_score_capped(name, None, q, 100)
}

type Wide<'a> = Option<&'a [u8; 64]>;

fn fuzzy_score_capped(name: &[u8], wide: Wide, q: &[u8], cap: i32) -> Option<i32> {
    if let Some(s) = prefix_score(name, q, cap) {
        return Some(s);
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = wide;
    #[cfg(target_arch = "aarch64")]
    if name.len() <= 64 && q.len() <= 32 {
        if let Some(w) = wide {
            return fuzzy_masked(name, w, q, cap);
        }
        let mut buf = [0u8; 64];
        buf[..name.len()].copy_from_slice(name);
        return fuzzy_masked(name, &buf, q, cap);
    }
    let mut end = 0;
    let mut from = 0;
    for &c in q {
        end = from + find_folded(&name[from..], c)?;
        from = end + 1;
    }
    let next = |k: usize, from: usize, to: usize| Some(from + find_folded(&name[from..=to], q[k])?);
    fuzzy_from(name, q, cap, end, next, |k, to| rfind_folded(&name[..to], q[k]))
}

#[cfg(target_arch = "aarch64")]
fn fuzzy_masked(name: &[u8], src: &[u8; 64], q: &[u8], cap: i32) -> Option<i32> {
    use std::arch::aarch64::*;
    let valid = u64::MAX.checked_shr(64 - name.len() as u32).unwrap_or(0);
    // Safety (here and below): NEON is part of aarch64; loads stay in `src`.
    let v: [uint8x16_t; 4] = std::array::from_fn(|i| unsafe {
        let x = vld1q_u8(src.as_ptr().add(16 * i));
        vorrq_u8(x, vandq_u8(vcltq_u8(vsubq_u8(x, vdupq_n_u8(b'A')), vdupq_n_u8(26)), vdupq_n_u8(0x20)))
    });
    const W: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];
    let weight = unsafe { vld1q_u8(W.as_ptr()) };
    let places = |c: u8| unsafe {
        let c = vdupq_n_u8(c);
        let m = |i: usize| vandq_u8(vceqq_u8(v[i], c), weight);
        let s = vpaddq_u8(vpaddq_u8(m(0), m(1)), vpaddq_u8(m(2), m(3)));
        vgetq_lane_u64(vreinterpretq_u64_u8(vpaddq_u8(s, s)), 0) & valid
    };
    let mut masks = [0u64; 32];
    let (mut end, mut from) = (0, 0);
    for i in 0..q.len() {
        let c = q[i];
        let m = if let Some(prev) = q[..i].iter().rposition(|&x| x == c) { masks[prev] } else { places(c) };
        masks[i] = m;
        let x = if from < 64 { m >> from << from } else { 0 };
        if x == 0 {
            return None;
        }
        end = x.trailing_zeros() as usize;
        from = end + 1;
    }
    let next = |k: usize, from: usize, to: usize| {
        let x = masks[k] >> from << from & u64::MAX >> (63 - to);
        (x != 0).then(|| x.trailing_zeros() as usize)
    };
    let prev = |k: usize, to: usize| {
        let x = masks[k] & u64::MAX.checked_shr(64 - to as u32).unwrap_or(0);
        (x != 0).then(|| 63 - x.leading_zeros() as usize)
    };
    fuzzy_from(name, q, cap, end, next, prev)
}

#[inline(always)]
fn fuzzy_from(
    name: &[u8],
    q: &[u8],
    cap: i32,
    end: usize,
    next: impl Fn(usize, usize, usize) -> Option<usize>,
    prev: impl Fn(usize, usize) -> Option<usize>,
) -> Option<i32> {
    if q.len() == 1 {
        return Some(single_score(name, end, cap));
    }
    let mut start = end + 1;
    for k in (0..q.len()).rev() {
        start = prev(k, start)?;
    }
    let mut score = 0;
    let (mut at, mut first_bonus) = (start, 0);
    for k in 0..q.len() {
        let mut run = false;
        if k > 0 {
            let last = at;
            at = next(k, last + 1, end)?;
            run = at == last + 1;
            if !run {
                score += GAP_START + (at - last - 2) as i32 * GAP_EXT;
            }
        }
        let prev = if at == 0 { Class::Delim } else { class(name[at - 1]) };
        let mut b = bonus(prev, class(name[at]));
        if run {
            if b >= BONUS_BOUNDARY && b > first_bonus {
                first_bonus = b;
            }
            b = b.max(first_bonus).max(BONUS_CONSEC);
        } else {
            first_bonus = b;
        }
        score += SCORE_MATCH + if k == 0 { b * 2 } else { b };
    }
    let off = (name.len() > 1 && name[0] == b'.') as usize;
    let at_stem = || end + 1 == name.iter().rposition(|&b| b == b'.').filter(|&p| p > off).unwrap_or(name.len());
    let placed = if start != off || end + 1 - start != q.len() {
        0
    } else if end + 1 == name.len() {
        100
    } else if at_stem() {
        80
    } else {
        30
    };
    Some(score + placed.min(cap) - (name.len() as i32).min(80) / 3)
}

#[inline]
fn prefix_score(name: &[u8], q: &[u8], cap: i32) -> Option<i32> {
    let off = (name.len() > 1 && name[0] == b'.') as usize;
    let end = off + q.len();
    if (off == 1 && q[0] == b'.') || name.len() < end || !name[off..end].iter().zip(q).all(|(&a, &b)| fold(a) == b) {
        return None;
    }
    let mut first = bonus(Class::Delim, class(name[off]));
    let mut score = SCORE_MATCH + first * 2;
    for i in off + 1..end {
        let b = bonus(class(name[i - 1]), class(name[i]));
        if b >= BONUS_BOUNDARY && b > first {
            first = b;
        }
        score += SCORE_MATCH + b.max(first).max(BONUS_CONSEC);
    }
    let placed = if end == name.len() {
        100
    } else if name[end] == b'.' && !name[end + 1..].contains(&b'.') {
        80
    } else {
        30
    };
    Some(score + placed.min(cap) - (name.len() as i32).min(80) / 3)
}

#[inline(always)]
fn find_folded(s: &[u8], c: u8) -> Option<usize> {
    if c.is_ascii_lowercase() { memchr::memchr2(c, c - 32, s) } else { memchr::memchr(c, s) }
}

#[inline(always)]
fn rfind_folded(s: &[u8], c: u8) -> Option<usize> {
    if c.is_ascii_lowercase() { memchr::memrchr2(c, c - 32, s) } else { memchr::memrchr(c, s) }
}

#[inline]
fn single_score(name: &[u8], i: usize, cap: i32) -> i32 {
    let prev = if i == 0 { Class::Delim } else { class(name[i - 1]) };
    let mut score = SCORE_MATCH + bonus(prev, class(name[i])) * 2;
    let off = (name.len() > 1 && name[0] == b'.') as usize;
    if i == off {
        let stem = name.iter().rposition(|&b| b == b'.').filter(|&p| p > off).unwrap_or(name.len());
        score += cap.min(if i + 1 == name.len() {
            100
        } else if i + 1 == stem {
            80
        } else {
            30
        });
    }
    score - (name.len() as i32).min(80) / 3
}

fn typo_score(name: &[u8], wide: Wide, m: u64, q: &[u8]) -> Option<i32> {
    let mut best = typo_at(name, wide, (name.len() > 1 && name[0] == b'.') as usize, q);
    if m & char_bit(b' ') != 0 {
        for sp in memchr::memchr_iter(b' ', name) {
            best = best.max(typo_at(name, wide, sp + 1, q));
        }
    }
    best
}

fn typo_at(name: &[u8], wide: Wide, s: usize, q: &[u8]) -> Option<i32> {
    if name.get(s).is_none_or(|&b| fold(b) != q[0]) {
        return None;
    }
    let mut fixed = [0u8; 128];
    let fixed = fixed.get_mut(..one_edit_prefix(&name[s..], q)?)?;
    for (f, &b) in fixed.iter_mut().zip(&name[s..]) {
        *f = fold(b);
    }
    Some(fuzzy_score_capped(name, wide, fixed, 30)? - TYPO_COST)
}

/// Digits are never edited: "hat_18" is not a typo of "hat_98".
fn one_edit_prefix(w: &[u8], q: &[u8]) -> Option<usize> {
    let starts = |w: &[u8], q: &[u8]| w.len() >= q.len() && w.iter().zip(q).all(|(&a, &b)| fold(a) == b);
    let i = (0..q.len()).find(|&i| i >= w.len() || fold(w[i]) != q[i])?;
    if q[i].is_ascii_digit() || w.get(i).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let rest = &q[i + 1..];
    let after = w.get(i + 1..).unwrap_or_default();
    if i + 1 < q.len() && i + 1 < w.len() && fold(w[i]) == q[i + 1] && fold(w[i + 1]) == q[i] && starts(&w[i + 2..], &q[i + 2..]) {
        return Some(q.len());
    }
    if i < w.len() && starts(after, rest) {
        return Some(q.len());
    }
    if starts(&w[i..], rest) {
        return Some(q.len() - 1);
    }
    (i < w.len() && starts(after, &q[i..])).then_some(q.len() + 1)
}

#[inline]
fn token_score(name: &[u8], wide: Wide, m: u64, t: &Token) -> Option<i32> {
    match t.mode {
        Mode::Fuzzy if takes_typos(&t.text, t.mode) => {
            let clean = if t.mask & !m == 0 { fuzzy_score_capped(name, wide, &t.text, 100) } else { None };
            clean.max(if m & t.start != 0 { typo_score(name, wide, m, &t.text) } else { None })
        }
        Mode::Fuzzy => fuzzy_score_capped(name, wide, &t.text, 100),
        Mode::Exact => find_ci(name, &t.text).map(|p| 40 + if p == 0 { 30 } else { 0 } - (name.len() as i32).min(80) / 3),
        Mode::Prefix => {
            (name.len() >= t.text.len() && name.iter().zip(&t.text).all(|(&a, &b)| fold(a) == b)).then(|| 60 - (name.len() as i32).min(80) / 3)
        }
        Mode::Suffix => (name.len() >= t.text.len() && name[name.len() - t.text.len()..].iter().zip(&t.text).all(|(&a, &b)| fold(a) == b))
            .then(|| 50 - (name.len() as i32).min(80) / 3),
    }
}

#[inline]
fn token_matches(name: &[u8], t: &Token) -> bool {
    match t.mode {
        Mode::Fuzzy => is_subseq(name, &t.text),
        _ => token_score(name, None, !0, t).is_some(),
    }
}

struct TopK {
    k: usize,
    buf: Vec<u64>,
    floor: u64,
}

#[inline(always)]
fn key(score: i32, i: u32) -> u64 {
    (((score as i64 - i32::MIN as i64) as u64) << 32) | (!i) as u64
}

impl TopK {
    fn new(k: usize, floor: u64) -> TopK {
        TopK { k, buf: Vec::new(), floor: if k == 0 { u64::MAX } else { floor } }
    }
    #[inline]
    fn push(&mut self, key: u64) {
        if key > self.floor {
            self.buf.push(key);
            if self.buf.len() >= (2 * self.k).max(64) {
                self.cut();
            }
        }
    }
    fn cut(&mut self) {
        if self.buf.len() > self.k {
            self.buf.select_nth_unstable_by(self.k - 1, |a, b| b.cmp(a));
            self.buf.truncate(self.k);
            self.floor = self.buf[self.k - 1];
        }
    }
}

fn par_each<S, T: Send>(n: usize, state: impl Fn() -> S + Sync, f: impl Fn(&mut S, usize) -> T + Sync) -> Vec<T> {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let out = Cells::new((0..n).map(|_| None));
    let next = AtomicUsize::new(0);
    let work = || {
        let mut st = state();
        loop {
            let i = next.fetch_add(1, Relaxed);
            if i >= n {
                break;
            }
            // Safety: the counter hands each index to one thread only.
            unsafe { out.set(i, Some(f(&mut st, i))) };
        }
    };
    let left = AtomicUsize::new((rayon::current_num_threads() - 1).min(n.saturating_sub(1)));
    fn helper<'s>(s: &rayon::Scope<'s>, left: &'s AtomicUsize, next: &'s AtomicUsize, n: usize, work: &'s (dyn Fn() + Sync)) {
        s.spawn(move |s| {
            if next.load(Relaxed) < n {
                if left.fetch_sub(1, Relaxed) > 1 {
                    helper(s, left, next, n, work);
                }
                work();
            }
        });
    }
    rayon::scope(|s| {
        if left.load(Relaxed) > 0 {
            helper(s, &left, &next, n, &work);
        }
        work();
    });
    out.0.into_iter().map(|x| x.into_inner().unwrap()).collect()
}

struct Cells<T>(Vec<std::cell::UnsafeCell<T>>);

unsafe impl<T: Send> Sync for Cells<T> {}

impl<T> Cells<T> {
    fn new(v: impl Iterator<Item = T>) -> Cells<T> {
        Cells(v.map(std::cell::UnsafeCell::new).collect())
    }

    /// Safety: no other thread uses index `i` meanwhile.
    unsafe fn set(&self, i: usize, v: T) {
        unsafe { *self.0[i].get() = v };
    }

    /// Safety: no other thread uses index `i` meanwhile.
    unsafe fn take(&self, i: usize) -> T
    where
        T: Default,
    {
        unsafe { std::mem::take(&mut *self.0[i].get()) }
    }
}

fn top_k(n: usize, k: usize, visits: usize, floor: u64, visit: impl Fn(std::ops::Range<usize>, &mut TopK) + Sync) -> Vec<Hit> {
    let piece = |r: std::ops::Range<usize>| {
        let mut top = TopK::new(k, floor);
        visit(r, &mut top);
        top.cut();
        top
    };
    let tops: Vec<TopK> = if visits <= INLINE_VISITS {
        vec![piece(0..n)]
    } else {
        let pieces = (rayon::current_num_threads() * 4).min(n.max(1));
        let step = n.div_ceil(pieces).max(1);
        par_each(pieces, || (), |_, p| piece((p * step).min(n)..((p + 1) * step).min(n)))
    };
    let floor = tops.iter().filter(|t| t.buf.len() == k).map(|t| t.floor).max().unwrap_or(0);
    let mut keys: Vec<u64> = tops.into_iter().flat_map(|t| t.buf).filter(|&x| x >= floor).collect();
    if keys.len() > k {
        if k == 0 {
            return Vec::new();
        }
        keys.select_nth_unstable_by(k - 1, |a, b| b.cmp(a));
        keys.truncate(k);
    }
    keys.into_iter().map(|x| Hit { score: ((x >> 32) as i64 + i32::MIN as i64) as i32, idx: !(x as u32), over: None }).collect()
}

fn ext_ok(name: &[u8], exts: &[Vec<u8>]) -> bool {
    let Some(dot) = name.iter().rposition(|&b| b == b'.') else { return false };
    let e = &name[dot + 1..];
    exts.iter().any(|x| x.len() == e.len() && x.iter().zip(e).all(|(&a, &b)| a == fold(b)))
}

const SELECTIVE: usize = 60_000;
const CHUNK_WORDS: usize = 64;
const INLINE_WORDS: usize = 4096;
const INLINE_VISITS: usize = 4096;
const RANKED_MIN: f64 = 20_000.0;
const BORDERLINE: f64 = 1_000.0;
const DRIVEN_NAMES: f64 = 20_000.0;
const DRIVEN_TRIES: usize = 2;
const BINS: usize = 1024;
const BIN0: i32 = 256;
const FIRST_BATCH: usize = 64;
const LAST_BATCH: usize = 4096;
const INLINE_NAMES: usize = 512;
const OVERLAY_CHUNK: usize = 256;

pub struct Searcher<'a> {
    pub live: &'a Live,
}

impl Searcher<'_> {
    pub fn scope_range(&self, q: &Query) -> Option<(usize, usize)> {
        let idx = &self.live.base;
        let Some(scope) = &q.scope else { return Some((1, idx.n)) };
        let e = idx.lookup(scope)?;
        let d = idx.dir_of(e)? as usize;
        Some((idx.dir_start()[d] as usize, idx.dir_end()[d] as usize))
    }

    pub fn search(&self, q: &Query) -> Vec<Hit> {
        let mut hits = self.search_base(q);
        hits.extend(self.search_overlay(q));
        hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.idx.cmp(&b.idx)));
        hits.truncate(q.limit);
        hits
    }

    fn search_base(&self, q: &Query) -> Vec<Hit> {
        let Some((lo, hi)) = self.scope_range(q) else { return Vec::new() };
        if let Some((t, tentative)) = self.rankable(q, lo, hi) {
            let scope = (q.scope.is_some() && hi - lo <= self.live.base.u / 4).then(|| self.scope_names(lo, hi));
            if let Some(hits) = self.ranked(q, t, lo, hi, scope.as_deref(), tentative) {
                return hits;
            }
        }
        let pos: Vec<&Token> = q.tokens.iter().filter(|t| !t.negate).collect();
        let neg: Vec<&Token> = q.tokens.iter().filter(|t| t.negate).collect();
        if (pos.len() > 1 || !neg.is_empty())
            && let Some(hits) = self.driven(q, &pos, &neg, lo, hi)
        {
            return hits;
        }
        let scored = self.names(q, &pos, &neg, lo, hi);
        let s = Scan { q, live: self.live, names: &scored.names, npos: pos.len(), need_dirs: pos.len() > 1 || !neg.is_empty(), now: now_secs() };
        let fast = !scored.names.all && !FULL_PASS.load(std::sync::atomic::Ordering::Relaxed);
        if fast && scored.names.ok_entries <= SELECTIVE {
            return s.selective(lo, hi);
        }

        let memo = if s.need_dirs { Some(scored.memo.get_or_init(|| self.dir_tokens(&scored.names))) } else { None };
        s.full(lo, hi, memo.map(|m| m.as_slice()))
    }

    fn rankable<'q>(&self, q: &'q Query, lo: usize, hi: usize) -> Option<(&'q Token, bool)> {
        let mut pos = q.tokens.iter().filter(|t| !t.negate);
        let (Some(t), None) = (pos.next(), pos.next()) else { return None };
        if t.mode != Mode::Fuzzy || q.kind.is_some() || q.size != (0, u64::MAX) || q.mtime != (0, u32::MAX) || q.path_re.is_some() || q.limit == 0 {
            return None;
        }
        let scope = (q.scope.is_some() && hi - lo <= self.live.base.u / 4).then_some((lo, hi));
        let key = NameKey::of(q, scope);
        if self.live.names_cache.last.lock().unwrap().as_ref().is_some_and(|p| key.narrows(&p.key)) {
            return None;
        }
        let (idx, u) = (&self.live.base, self.live.base.u as f64);
        let counts = idx.class_counts();
        let est = (0..CLASSES).filter(|&c| t.mask & (1 << c) != 0).fold(u, |e, c| e * counts[c] as f64 / u);
        if est >= RANKED_MIN {
            return Some((t, false));
        }
        if est < BORDERLINE || q.scope.is_some() || !t.text.iter().all(u8::is_ascii_alphabetic) {
            return None;
        }
        let tb = TokenBits::new(idx, t);
        let found = (0..idx.words).step_by(64).map(|w| tb.word(w).0.count_ones() as f64).sum::<f64>() * 64.0;
        (found >= RANKED_MIN).then_some((t, true))
    }

    fn ranked(&self, q: &Query, t: &Token, lo: usize, hi: usize, within: Option<&NameSet>, tentative: bool) -> Option<Vec<Hit>> {
        let idx = &self.live.base;
        let (prior, info, ne_off, ne) = (idx.name_prior(), idx.name_info(), idx.name_ents_off(), idx.name_ents());
        let tb = TokenBits::new(idx, t);
        let start = idx.bitmap(BM_START + crate::index::start_hash(t.text[0]));
        let space = space_bits(idx);
        let tweak = |flags: u8| 10 + if flags & NF_APP != 0 { 25 } else { 0 } - if flags & NF_DOT != 0 { 8 } else { 0 };
        let neg: Vec<&Token> = q.tokens.iter().filter(|t| t.negate).collect();
        let scoped = within.is_some();
        let within = |w: usize| within.map_or(!0, |s| s.bits[w]);
        let score = |k: u32, (_, clean, typo): (u64, u64, u64), re: Option<&regex::bytes::Regex>| -> Option<Cand> {
            let (w, i) = (k as usize / 64, k % 64);
            let name = idx.uname(k);
            let spaced = (space[0][w] | space[1][w]) >> i & 1 != 0;
            let s = token_score(name, idx.uname_wide(k), t.known_mask(clean >> i & 1 != 0, typo >> i & 1 != 0, spaced), t)?;
            if !(q.exts.is_empty() || ext_ok(name, &q.exts)) || re.is_some_and(|re| !re.is_match(name)) || neg.iter().any(|t| token_matches(name, t))
            {
                return None;
            }
            let score = s.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let flags = name_flags(name) | NF_OK;
            Some(Cand { bound: score as i32 + prior[k as usize] as i32 + tweak(flags), k, score, flags })
        };
        let (m, t0) = (t.text.len() as i32, t.text[0]);
        let base = 24 * m + 8 - if class(t0) == Class::Delim { 16 } else { 0 };
        let cheap = |x: crate::index::NameInfo| {
            base + x.key as i32
                + if x.head != t0 {
                    0
                } else if x.len as i32 == m {
                    100
                } else if x.stem as i32 == m {
                    80
                } else {
                    30
                }
        };
        let scan = Scan {
            q,
            live: self.live,
            names: &NameTable::new(Vec::new(), Vec::new(), Vec::new(), 0, false),
            npos: 1,
            need_dirs: false,
            now: now_secs(),
        };
        let entries = |k: u32| &ne[ne_off[k as usize] as usize..ne_off[k as usize + 1] as usize];
        let (parent, dir_prior, de, dp, en) = (idx.parent(), idx.dir_prior(), idx.dir_entry(), idx.dir_parent(), idx.ent_name());
        let negated = |d: u32, cache: &mut HashMap<u32, bool, crate::index::Fx>| {
            let mut chain = Vec::new();
            let mut k = d;
            let mut out = loop {
                if k == 0 {
                    break false;
                }
                if let Some(&b) = cache.get(&k) {
                    break b;
                }
                chain.push(k);
                k = dp[k as usize];
            };
            for &k in chain.iter().rev() {
                out = out || neg.iter().any(|t| token_matches(idx.uname(en[de[k as usize] as usize]), t));
                cache.insert(k, out);
            }
            out
        };
        let visit = |names: &[Cand], floor: u64| {
            let visits = names.iter().map(|x| entries(x.k).len().min(q.limit)).sum();
            top_k(names.len(), q.limit, visits, floor, |r, top| {
                let (mut pbuf, mut cache) = (Vec::new(), HashMap::default());
                for c in &names[r] {
                    let nh = NameHit { score: c.score, bits: 1, flags: c.flags, best: [0; 4] };
                    for &e in entries(c.k).iter().filter(|&&e| (lo..hi).contains(&(e as usize))) {
                        if key(c.score as i32 + dir_prior[parent[e as usize] as usize] as i32 + tweak(c.flags), 0) <= top.floor {
                            break;
                        }
                        if !neg.is_empty() && negated(parent[e as usize], &mut cache) {
                            continue;
                        }
                        if let Some(key) = scan.score(e as usize, nh, DirMemo::default(), top.floor, &mut pbuf, None) {
                            top.push(key);
                        }
                    }
                }
            })
        };
        let best = |mut hits: Vec<Hit>| {
            hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.idx.cmp(&b.idx)));
            hits.truncate(q.limit);
            let last = hits.get(q.limit - 1).map(|h| (h.score, key(h.score, h.idx)));
            let (floor, fkey) = last.unwrap_or((i32::MIN, 0));
            (hits, floor, fkey)
        };
        let bin = |b: i32| (b + BIN0).clamp(0, BINS as i32 - 1) as usize;
        let sorted = |got: Vec<(i32, u32)>, h: Vec<u32>| {
            let mut at = vec![0u32; BINS];
            let mut acc = 0;
            for b in (0..BINS).rev() {
                at[b] = acc;
                acc += h[b];
            }
            let mut sorted = vec![(0, 0); got.len()];
            for x in got {
                let b = bin(x.0);
                sorted[at[b] as usize] = x;
                at[b] += 1;
            }
            (sorted, h)
        };
        let by_key = idx.by_key();
        struct Run {
            hits: Vec<Hit>,
            floor: i32,
            fkey: u64,
            top: usize,
            want: usize,
            listed: usize,
        }
        type Part = (Vec<(i32, u32)>, Vec<u32>);
        let run = |parts: Vec<Part>, complete: bool, r: Run| -> Result<Option<Vec<Hit>>, Run> {
            let Run { mut hits, mut floor, mut fkey, mut top, mut want, mut listed } = r;
            let mut hist = vec![0usize; BINS];
            for (_, h) in &parts {
                hist.iter_mut().zip(h).for_each(|(a, &b)| *a += b as usize);
            }
            for (key, &n) in (by_key.cover..=i8::MAX as i32).zip(&by_key.counts) {
                hist[bin(base + key)] += n as usize;
            }
            let mut taken: Vec<usize> = parts.iter().map(|(items, _)| items.iter().take_while(|x| bin(x.0) >= top).count()).collect();
            let done = |floor: i32, top: usize| floor != i32::MIN && (top == 0 || floor > top as i32 - 1 - BIN0);
            let least = if complete { 0 } else { bin(base + by_key.cover + 30) };
            while want <= LAST_BATCH && !done(floor, top) && top > least {
                let low = if floor == i32::MIN { 0 } else { bin(floor) }.max(least);
                let (mut cut, mut n) = (top, 0);
                while cut > low && n < want {
                    cut -= 1;
                    n += hist[cut];
                }
                let ends: Vec<usize> =
                    parts.iter().zip(&taken).map(|((items, _), &a)| a + items[a..].iter().take_while(|x| bin(x.0) >= cut).count()).collect();
                let lend = listed + by_key.list[listed..].iter().take_while(|x| bin(base + x.0 as i32) >= cut).count();
                let one = |p: usize| {
                    let re = q.name_re.clone();
                    let items = &parts[p].0[taken[p]..ends[p]];
                    items
                        .iter()
                        .filter_map(|&(_, k)| score(k, tb.word(k as usize / 64), re.as_ref()).filter(|c| c.bound >= floor))
                        .collect::<Vec<_>>()
                };
                let mut batch: Vec<Cand> = if n <= INLINE_NAMES {
                    (0..parts.len()).flat_map(one).collect()
                } else {
                    par_each(parts.len(), || (), |_, p| one(p)).into_iter().flatten().collect()
                };
                let re = q.name_re.clone();
                batch.extend(by_key.list[listed..lend].iter().filter_map(|&(_, k)| {
                    let (w, i) = (k as usize / 64, k % 64);
                    if info[k as usize].head == t0 || within(w) >> i & 1 == 0 {
                        return None;
                    }
                    let x = tb.word(w);
                    (x.0 >> i & 1 != 0).then(|| score(k, x, re.as_ref())).flatten().filter(|c| c.bound >= floor)
                }));
                (taken, listed) = (ends, lend);
                (hits, floor, fkey) = best(hits.into_iter().chain(visit(&batch, fkey)).collect());
                top = cut;
                want *= 4;
                if tentative && !(done(floor, top) && floor >= base + by_key.cover) || hits.len() < q.limit / 4 && want > FIRST_BATCH * 4 {
                    if !complete {
                        break;
                    }
                    return Ok(None);
                }
            }
            if done(floor, top) && floor >= base + by_key.cover && (complete || floor > base + by_key.cover + 29) {
                return Ok(Some(hits));
            }
            if !complete {
                return Err(Run { hits, floor, fkey, top, want, listed });
            }
            if floor == i32::MIN {
                return Ok(None);
            }
            let scored = |k: usize| {
                let x = info[k];
                let placed = x.head == t0;
                bin(cheap(x)) >= top && (placed || x.key as i32 >= by_key.cover)
            };
            let rest = par_each(
                idx.words.div_ceil(CHUNK_WORDS),
                || q.name_re.clone(),
                |re, c| {
                    let mut out = Vec::new();
                    for w in c * CHUNK_WORDS..((c + 1) * CHUNK_WORDS).min(idx.words) {
                        let mut m = within(w);
                        if m == 0 {
                            continue;
                        }
                        let x = tb.word(w);
                        m &= x.0;
                        while m != 0 {
                            let k = w * 64 + m.trailing_zeros() as usize;
                            m &= m - 1;
                            if cheap(info[k]) >= floor
                                && !scored(k)
                                && let Some(c) = score(k as u32, x, re.as_ref()).filter(|c| c.bound >= floor)
                            {
                                out.push(c);
                            }
                        }
                    }
                    out
                },
            )
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            Ok(Some(best(hits.into_iter().chain(visit(&rest, fkey)).collect()).0))
        };
        let mut from = Run { hits: Vec::new(), floor: i32::MIN, fkey: 0, top: BINS, want: FIRST_BATCH, listed: 0 };
        if t.text.len() <= crate::index::SHORT && !scoped {
            let m = t.text.len();
            let whole = |k: &u32| {
                let x = info[*k as usize];
                (x.len as usize == m || x.stem as usize == m) && (x.key as i32) < by_key.cover
            };
            let (mut got, mut h) = (Vec::new(), vec![0u32; BINS]);
            for &k in by_key.heads[t0 as usize].iter().chain(by_key.short[t0 as usize].iter().filter(|k| whole(k))) {
                if tb.word(k as usize / 64).0 >> (k % 64) & 1 != 0 {
                    let c = cheap(info[k as usize]);
                    h[bin(c)] += 1;
                    got.push((c, k));
                }
            }
            match run(vec![sorted(got, h)], false, from) {
                Ok(found) => return found,
                Err(r) => from = r,
            }
        }
        let pieces = if idx.words <= INLINE_WORDS { 1 } else { rayon::current_num_threads() * 2 };
        let step = idx.words.div_ceil(pieces);
        let piece = |p: usize| {
            let (mut got, mut h) = (Vec::new(), vec![0u32; BINS]);
            for (w, &s) in start.iter().enumerate().take(((p + 1) * step).min(idx.words)).skip(p * step) {
                let mut x = s & within(w);
                if x == 0 {
                    continue;
                }
                x &= tb.word(w).0;
                while x != 0 {
                    let k = w * 64 + x.trailing_zeros() as usize;
                    x &= x - 1;
                    if info[k].head == t0 {
                        let c = cheap(info[k]);
                        h[bin(c)] += 1;
                        got.push((c, k as u32));
                    }
                }
            }
            sorted(got, h)
        };
        let parts = if pieces == 1 { vec![piece(0)] } else { par_each(pieces, || (), |_, p| piece(p)) };
        run(parts, true, from).ok().flatten()
    }

    fn driven(&self, q: &Query, pos: &[&Token], neg: &[&Token], lo: usize, hi: usize) -> Option<Vec<Hit>> {
        let idx = &self.live.base;
        let (counts, u) = (idx.class_counts(), idx.u as f64);
        let est = |t: &Token| (0..CLASSES).filter(|&c| t.mask & (1 << c) != 0).fold(u, |e, c| e * counts[c] as f64 / u);
        let mut order: Vec<usize> = (0..pos.len()).filter(|&t| est(pos[t]) <= DRIVEN_NAMES).collect();
        order.sort_by(|&a, &b| est(pos[a]).total_cmp(&est(pos[b])));
        for t in order.into_iter().take(DRIVEN_TRIES) {
            match self.drive(q, pos, neg, lo, hi, t) {
                Ok(hits) => return Some(hits),
                Err(true) => continue,
                Err(false) => return None,
            }
        }
        None
    }

    fn drive(&self, q: &Query, pos: &[&Token], neg: &[&Token], lo: usize, hi: usize, t: usize) -> Result<Vec<Hit>, bool> {
        let idx = &self.live.base;
        let tb = TokenBits::new(idx, pos[t]);
        let space = space_bits(idx);
        let (ne_off, ne, kind, en, parent) = (idx.name_ents_off(), idx.name_ents(), idx.kind(), idx.ent_name(), idx.parent());
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
        let chunks = idx.words.div_ceil(CHUNK_WORDS);
        let (seen, quit, early) = (AtomicUsize::new(0), AtomicBool::new(false), AtomicBool::new(false));
        let over = || quit.load(Relaxed);
        let spaced = |w: usize, i: u32| (space[0][w] | space[1][w]) >> i & 1 != 0;
        let found = par_each(
            chunks,
            || (),
            |_, c| {
                let (mut ents, mut trees) = (Vec::new(), Vec::new());
                for w in c * CHUNK_WORDS..((c + 1) * CHUNK_WORDS).min(idx.words) {
                    if over() {
                        break;
                    }
                    let (fits, clean, typo) = tb.word(w);
                    let mut m = fits;
                    while m != 0 {
                        let i = m.trailing_zeros();
                        m &= m - 1;
                        let k = w * 64 + i as usize;
                        let known = pos[t].known_mask(clean >> i & 1 != 0, typo >> i & 1 != 0, spaced(w, i));
                        if token_score(idx.uname(k as u32), idx.uname_wide(k as u32), known, pos[t]).is_none() {
                            continue;
                        }
                        let mut n = 0;
                        for &e in &ne[ne_off[k] as usize..ne_off[k + 1] as usize] {
                            if n > SELECTIVE {
                                break;
                            }
                            if (lo..hi).contains(&(e as usize)) {
                                ents.push(e);
                                n += 1;
                            }
                            if kind[e as usize] & 3 == KIND_DIR
                                && let Some(d) = idx.dir_of(e)
                            {
                                let (a, b) = (idx.dir_start()[d as usize].max(lo as u32), idx.dir_end()[d as usize].min(hi as u32));
                                if a < b {
                                    trees.push((a, b));
                                    n += (b - a) as usize;
                                }
                            }
                        }
                        if seen.fetch_add(n, Relaxed) + n > SELECTIVE {
                            quit.store(true, Relaxed);
                            early.fetch_or(c < chunks / 16, Relaxed);
                        }
                    }
                }
                if c >= chunks / 16 && seen.load(Relaxed) * chunks / (c + 1) > 2 * SELECTIVE {
                    quit.store(true, Relaxed);
                }
                (ents, trees)
            },
        );
        let (mut ents, mut trees): (Vec<u32>, Vec<(u32, u32)>) = (Vec::new(), Vec::new());
        for (e, t) in found {
            ents.extend(e);
            trees.extend(t);
        }
        if over() {
            return Err(early.load(Relaxed));
        }
        trees.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (a, b) in trees {
            match merged.last_mut() {
                Some(l) if a <= l.1 => l.1 = l.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let first = ents.len();
        if first + merged.iter().map(|&(a, b)| (b - a) as usize).sum::<usize>() > SELECTIVE {
            return Err(false);
        }
        ents.extend(merged.iter().flat_map(|&(a, b)| a..b));
        if ents.len() <= INLINE_VISITS {
            let scan = Scan {
                q,
                live: self.live,
                names: &NameTable::new(Vec::new(), Vec::new(), Vec::new(), 0, false),
                npos: pos.len(),
                need_dirs: true,
                now: now_secs(),
            };
            return Ok(top_k(ents.len(), q.limit, ents.len(), 0, |r, top| {
                let (mut pbuf, mut memo) = (Vec::new(), HashMap::<u32, DirMemo, crate::index::Fx>::default());
                let mut hits = HashMap::<u32, Option<NameHit>, crate::index::Fx>::default();
                let re = q.name_re.clone();
                let mut hit = |k: u32| *hits.entry(k).or_insert_with(|| name_hit(idx, q, pos, neg, re.as_ref(), k));
                for j in r {
                    let i = ents[j] as usize;
                    let Some(nh) = hit(en[i]).filter(|h| h.flags & NF_OK != 0 && (j < first || h.bits >> t & 1 == 0)) else { continue };
                    let m = scan.memo_of(parent[i], &mut memo, &mut hit);
                    if let Some(key) = scan.score(i, nh, m, top.floor, &mut pbuf, q.path_re.as_ref()) {
                        top.push(key);
                    }
                }
            }));
        }
        let (de, dp) = (idx.dir_entry(), idx.dir_parent());
        let (mut bits, mut seen_dir) = (vec![0u64; idx.words], vec![0u64; idx.d.div_ceil(64)]);
        for &e in &ents {
            let k = en[e as usize];
            bits[k as usize >> 6] |= 1 << (k & 63);
            let mut d = parent[e as usize];
            while d != 0 && seen_dir[d as usize >> 6] >> (d & 63) & 1 == 0 {
                seen_dir[d as usize >> 6] |= 1 << (d & 63);
                let k = en[de[d as usize] as usize];
                bits[k as usize >> 6] |= 1 << (k & 63);
                d = dp[d as usize];
            }
        }
        let names = self.score_names(q, pos, neg, None, Some(&NameSet { busy: true, ..NameSet::new(bits) }));
        let scan = Scan { q, live: self.live, names: &names, npos: pos.len(), need_dirs: true, now: now_secs() };
        Ok(top_k(ents.len(), q.limit, ents.len(), 0, |r, top| {
            let (mut pbuf, mut memo) = (Vec::new(), HashMap::<u32, DirMemo, crate::index::Fx>::default());
            let path_re = q.path_re.clone();
            for j in r {
                let i = ents[j] as usize;
                let Some(nh) = names.get(en[i]).filter(|h| h.flags & NF_OK != 0 && (j < first || h.bits >> t & 1 == 0)) else { continue };
                let m = scan.memo_of(parent[i], &mut memo, &mut |k| names.get(k));
                if let Some(key) = scan.score(i, nh, m, top.floor, &mut pbuf, path_re.as_ref()) {
                    top.push(key);
                }
            }
        }))
    }

    fn names(&self, q: &Query, pos: &[&Token], neg: &[&Token], lo: usize, hi: usize) -> std::sync::Arc<Scored> {
        let scope = (q.scope.is_some() && hi - lo <= self.live.base.u / 4).then_some((lo, hi));
        let key = NameKey::of(q, scope);
        let prev = self.live.names_cache.last.lock().unwrap().clone();
        if let Some(p) = &prev
            && p.key == key
        {
            return p.clone();
        }
        let from = prev.as_ref().filter(|p| key.narrows(&p.key)).map(|p| &p.names);
        let within = scope.filter(|_| from.is_none()).map(|(lo, hi)| self.scope_names(lo, hi));
        let scored = std::sync::Arc::new(Scored {
            at: std::time::Instant::now(),
            key,
            names: self.score_names(q, pos, neg, from, within.as_deref()),
            memo: std::sync::OnceLock::new(),
        });
        *self.live.names_cache.last.lock().unwrap() = Some(scored.clone());
        scored
    }

    fn scope_names(&self, lo: usize, hi: usize) -> std::sync::Arc<NameSet> {
        let mut cache = self.live.names_cache.scope.lock().unwrap();
        if let Some((r, set)) = &*cache
            && *r == (lo, hi)
        {
            return set.clone();
        }
        let idx = &self.live.base;
        let (en, de, dp) = (idx.ent_name(), idx.dir_entry(), idx.dir_parent());
        let mut bits = vec![0u64; idx.words];
        let mut set = |k: u32| bits[k as usize >> 6] |= 1 << (k & 63);
        for &k in &en[lo..hi] {
            set(k);
        }
        let mut d = if lo < hi { idx.parent()[lo] } else { 0 };
        while d != 0 {
            set(en[de[d as usize] as usize]);
            d = dp[d as usize];
        }
        let set = std::sync::Arc::new(NameSet::new(bits));
        *cache = Some(((lo, hi), set.clone()));
        set
    }

    fn score_names(&self, q: &Query, pos: &[&Token], neg: &[&Token], from: Option<&NameTable>, within: Option<&NameSet>) -> NameTable {
        let idx = &self.live.base;
        let tails = q.path_re.as_ref().and_then(path_suffixes);
        if pos.is_empty() && neg.is_empty() && q.exts.is_empty() && q.name_re.is_none() && tails.is_none() {
            return NameTable::new(Vec::new(), Vec::new(), Vec::new(), idx.n, true);
        }
        let ne_off = idx.name_ents_off();
        let by_ext = pos.len() == 1 && neg.is_empty() && !q.exts.is_empty();
        let toks: Vec<TokenBits> = pos.iter().chain(neg).map(|t| TokenBits::new(idx, t)).collect();
        let space = space_bits(idx);
        let filter = |lits: Vec<Vec<u8>>| lits.into_iter().map(Token::literal).collect::<Vec<_>>();
        let re_toks = q.name_re.as_ref().and_then(re_literals).map(filter);
        let re_bits = re_toks.as_ref().map(|t| t.iter().map(|t| TokenBits::new(idx, t)).collect::<Vec<_>>());
        let ext_slots = idx.name_ext();
        let mut allow = [0u64; 4];
        for e in &q.exts {
            let s = idx.ext_slot(e) as usize;
            allow[s / 64] |= 1 << (s % 64);
        }
        let ext_ok_k = |k: usize, name: &[u8]| {
            let s = ext_slots[k] as usize;
            allow[s / 64] >> (s % 64) & 1 != 0 && (s != 255 || ext_ok(name, &q.exts))
        };
        let ext_word = |w: usize| -> u64 {
            if q.exts.is_empty() {
                return !0;
            }
            let slots = &ext_slots[w * 64..(w * 64 + 64).min(idx.u)];
            slots.iter().enumerate().fold(0, |m, (i, &s)| m | (allow[s as usize / 64] >> (s % 64) & 1) << i)
        };
        let (dot, app) = (idx.bitmap(crate::index::BM_DOT), idx.bitmap(crate::index::BM_APP));
        let score_one =
            |k: usize, i: u32, per: &[Pair], j: usize, spaced: bool, re: Option<&regex::bytes::Regex>, ok_entries: &mut usize| -> Option<NameHit> {
                let name = idx.uname(k as u32);
                let (w, bit) = (k / 64, |x: u64| x >> i & 1 != 0);
                let flags =
                    if pos.is_empty() { (if bit(dot[w]) { NF_DOT } else { 0 }) | (if bit(app[w]) { NF_APP } else { 0 }) } else { name_flags(name) };
                let mut h = NameHit { score: 0, bits: 0, flags, best: [0; 4] };
                for (t, tok) in pos.iter().enumerate() {
                    let (fits, clean, typo) = per[t].0[j];
                    if bit(fits)
                        && let Some(s) = token_score(name, idx.uname_wide(k as u32), tok.known_mask(bit(clean), bit(typo), spaced), tok)
                    {
                        h.bits |= 1 << t;
                        let s16 = s.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                        h.score = h.score.saturating_add(s16);
                        if t < 4 {
                            h.best[t] = s16.max(0);
                        }
                    }
                }
                if neg.iter().zip(&per[pos.len()..]).any(|(t, p)| bit(p.0[j].0) && token_matches(name, t)) {
                    h.flags |= NF_NEG;
                }
                let ok = (pos.is_empty() || h.bits != 0)
                    && h.flags & NF_NEG == 0
                    && (q.exts.is_empty() || ext_ok_k(k, name))
                    && tails.as_ref().is_none_or(|t| t.iter().any(|t| ends_with_fold(name, t)))
                    && re.is_none_or(|re| re.is_match(name));
                if ok {
                    h.flags |= NF_OK;
                    *ok_entries += (ne_off[k + 1] - ne_off[k]) as usize;
                }
                (ok || h.bits != 0 || h.flags & NF_NEG != 0).then_some(h)
            };
        let chunk = |c: usize, bits: &mut [u64], rank: &mut [u16], mut hits: Vec<NameHit>, per: &mut [Pair]| {
            let mut ok = 0;
            let (lo, n) = (c * CHUNK_WORDS, bits.len());
            let mut todo: Todo = std::array::from_fn(|g| 1u64.checked_shl(n.saturating_sub(g * 64) as u32).map_or(!0, |b| b - 1));
            if let Some(f) = from {
                let words = if f.hits[c].is_empty() { Todo::default() } else { nonzero(&f.bits[lo..lo + n]) };
                todo = std::array::from_fn(|g| todo[g] & words[g]);
            }
            if let Some(s) = within {
                todo = std::array::from_fn(|g| todo[g] & s.todo[c][g]);
            }
            if todo == Todo::default() {
                return (hits, ok);
            }
            let re = q.name_re.clone();
            let cand_of = |w: usize| from.map_or(!0, |f| f.bits[w]) & within.map_or(!0, |s| s.bits[w]);
            let has = |todo: &[u64], wi: usize| wi < n && todo[wi / 64] >> (wi % 64) & 1 != 0;
            for g in 0..todo.len() {
                while todo[g] != 0 {
                    let wi = g * 64 + todo[g].trailing_zeros() as usize;
                    todo[g] &= todo[g] - 1;
                    let pair = has(&todo, wi + 1);
                    if pair {
                        todo[(wi + 1) / 64] &= !(1 << ((wi + 1) % 64));
                    }
                    let w = lo + wi;
                    for (p, tb) in per.iter_mut().zip(&toks) {
                        p.0 = if pair { tb.words::<2>(w) } else { [tb.word(w), (0, 0, 0)] };
                    }
                    for j in 0..1 + pair as usize {
                        let (wi, w) = (wi + j, w + j);
                        rank[wi] = hits.len() as u16;
                        let mut any = per.iter().take(toks.len()).fold(0, |a, p| a | p.0[j].0);
                        if pos.is_empty() {
                            let re = re_bits.as_ref().map_or(!0, |f| f.iter().fold(0, |a, t| a | t.word(w).0));
                            any |= !0 >> (64 - (idx.u - w * 64).min(64)) & ext_word(w) & re;
                        } else if by_ext && any != 0 {
                            any &= ext_word(w);
                        }
                        let mut cand = cand_of(w) & any;
                        while cand != 0 {
                            let i = cand.trailing_zeros();
                            cand &= cand - 1;
                            let spaced = (space[0][w] | space[1][w]) >> i & 1 != 0;
                            if let Some(h) = score_one(w * 64 + i as usize, i, per, j, spaced, re.as_ref(), &mut ok) {
                                hits.push(h);
                                bits[wi] |= 1 << i;
                            }
                        }
                    }
                }
            }
            (hits, ok)
        };
        let spare_bits = BITS_POOL.lock().unwrap().pop().filter(|(b, _)| b.len() == idx.words);
        let (mut bits, mut rank) = spare_bits.unwrap_or_else(|| (vec![0; idx.words], vec![0; idx.words]));
        let few = from.map_or(within.map_or(idx.words, |s| if s.busy { s.names } else { s.words }), |f| f.len) <= INLINE_WORDS;
        let mut spare = std::mem::take(&mut *HIT_POOL.lock().unwrap());
        spare.resize_with(idx.words.div_ceil(CHUNK_WORDS), Vec::new);
        let scratch = || vec![Line([(0, 0, 0); 2]); toks.len()];
        let chunks = bits.chunks_mut(CHUNK_WORDS).zip(rank.chunks_mut(CHUNK_WORDS)).zip(spare);
        let out: Vec<(Vec<NameHit>, usize)> = if few {
            let mut per = scratch();
            chunks.enumerate().map(|(c, ((b, r), h))| chunk(c, b, r, h, &mut per)).collect()
        } else {
            let slots = Cells::new(chunks.map(Some));
            par_each(slots.0.len(), scratch, |per, c| {
                // Safety: par_each runs each chunk once.
                let ((b, r), h) = unsafe { slots.take(c) }.unwrap();
                chunk(c, b, r, h, per)
            })
        };
        let ok_entries = out.iter().map(|c| c.1).sum();
        let hits: Vec<Vec<NameHit>> = out.into_iter().map(|c| c.0).collect();
        NameTable::new(bits, rank, hits, ok_entries, false)
    }

    fn search_overlay(&self, q: &Query) -> Vec<Hit> {
        let now = now_secs();
        let pos: Vec<&Token> = q.tokens.iter().filter(|t| !t.negate).collect();
        let flat = &self.live.flat;
        let cands: Vec<&(Vec<u8>, crate::live::OEnt)> = flat
            .masks
            .iter()
            .zip(&flat.items)
            .filter(|&(&m, _)| m != 0 && (pos.is_empty() || pos.iter().any(|t| t.fits(m))))
            .map(|(_, item)| item)
            .collect();
        if cands.is_empty() {
            return Vec::new();
        }
        let score = |items: &[&(Vec<u8>, crate::live::OEnt)]| {
            let mut memo = HashMap::<&[u8], DirMatch, crate::index::Fx>::default();
            let mut out = Vec::new();
            for &&(ref path, o) in items {
                let cut = path.iter().rposition(|&b| b == b'/').unwrap_or(0);
                let dir = &path[..cut];
                if let Some(score) = q.match_path_with(path, o.kind, o.size, o.mtime, |_| *memo.entry(dir).or_insert_with(|| q.dir_match(dir))) {
                    let score = score + o.prior as i32 + rank_tweaks(name_flags(&path[cut + 1..]), o.kind, o.mtime, now);
                    out.push(Hit { score, idx: u32::MAX, over: Some(path.clone()) });
                }
            }
            out
        };
        let mut hits = if cands.len() <= OVERLAY_CHUNK { score(&cands) } else { cands.par_chunks(OVERLAY_CHUNK).flat_map_iter(score).collect() };
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.over.cmp(&b.over)));
        hits.truncate(q.limit);
        hits
    }

    fn dir_tokens(&self, names: &NameTable) -> Vec<DirMemo> {
        let idx = &self.live.base;
        let de = idx.dir_entry();
        let en = idx.ent_name();
        let mut out = MEMO_POOL.lock().unwrap().pop().unwrap_or_default();
        (0..idx.d)
            .into_par_iter()
            .with_min_len(1 << 12)
            .map(|k| if k > 0 { DirMemo::own(names.get(en[de[k] as usize])) } else { DirMemo::default() })
            .collect_into_vec(&mut out);
        let dp = idx.dir_parent();
        let plan = idx.memo_plan();
        for &k in &plan.upper {
            out[k as usize] = out[k as usize].under(out[dp[k as usize] as usize]);
        }
        let roots: Vec<(u32, DirMemo)> = plan.chunks.iter().map(|(c, _)| (*c, out[*c as usize])).collect();
        let mut slices = Vec::with_capacity(plan.chunks.len());
        let mut rest: &mut [DirMemo] = &mut out;
        let mut at = 0usize;
        for (_, r) in &plan.chunks {
            let (_, tail) = std::mem::take(&mut rest).split_at_mut(r.start as usize - at);
            let (mine, tail) = tail.split_at_mut((r.end - r.start) as usize);
            slices.push(mine);
            rest = tail;
            at = r.end as usize;
        }
        slices.into_par_iter().zip(&plan.chunks).zip(roots).for_each(|((slice, (_, r)), (c, root))| {
            let a = r.start as usize;
            for k in a..r.end as usize {
                let p = dp[k];
                let pm = if p == c { root } else { slice[p as usize - a] };
                slice[k - a] = slice[k - a].under(pm);
            }
        });
        out
    }
}

pub static FULL_PASS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Scan<'a> {
    q: &'a Query,
    live: &'a Live,
    names: &'a NameTable,
    npos: usize,
    need_dirs: bool,
    now: u32,
}

impl Scan<'_> {
    #[inline(always)]
    fn score(&self, i: usize, nh: NameHit, memo: DirMemo, floor: u64, pbuf: &mut Vec<u8>, re: Option<&regex::bytes::Regex>) -> Option<u64> {
        let idx = &self.live.base;
        let q = self.q;
        let k = idx.kind()[i];
        if !q.kind_ok(k) {
            return None;
        }
        if q.size != (0, u64::MAX) {
            let sz = idx.size_of(i);
            if sz < q.size.0 || sz > q.size.1 {
                return None;
            }
        }
        let mtime = idx.mtime()[i];
        if q.mtime != (0, u32::MAX) && (mtime < q.mtime.0 || mtime > q.mtime.1) {
            return None;
        }
        let mut score = nh.score as i32;
        if self.need_dirs {
            let all = (1u32 << self.npos) - 1;
            if memo.bits == u32::MAX || (nh.bits as u32 | memo.bits) & all != all {
                return None;
            }
            for t in 0..self.npos {
                if nh.bits & (1 << t) == 0 {
                    score += memo.best.get(t).map_or(6, |&b| b as i32 * 3 / 4);
                }
            }
        }
        if self.live.is_dead(i as u32) {
            return None;
        }
        let p = idx.parent()[i] as usize;
        score += idx.dir_prior()[p] as i32 + rank_tweaks(nh.flags, k, mtime, self.now);
        let key = key(score, i as u32);
        if key <= floor {
            return None;
        }
        if let Some(re) = re {
            idx.path(i, pbuf);
            if !re.is_match(pbuf) {
                return None;
            }
        }
        Some(key)
    }

    fn full(&self, lo: usize, hi: usize, memo: Option<&[DirMemo]>) -> Vec<Hit> {
        let idx = &self.live.base;
        let (ent_name, parent, zones) = (idx.ent_name(), idx.parent(), idx.zones());
        let dense = self.names.dense(idx);
        let q = self.q;
        let filtered = q.kind.is_some() || q.size != (0, u64::MAX) || q.mtime != (0, u32::MAX);
        let zone_ok = |z: &Zone| {
            z.max_size >= q.size.0
                && z.max_mtime >= q.mtime.0
                && z.min_mtime <= q.mtime.1
                && q.kind.is_none_or(|k| z.kinds & (1 << k) != 0 || (q.apps && z.kinds & (1 << KIND_LINK) != 0))
        };
        top_k(hi - lo, q.limit, hi - lo, 0, |r, top| {
            let (mut pbuf, re) = (Vec::new(), q.path_re.clone());
            let (mut a, b) = (lo + r.start, lo + r.end);
            while a < b {
                let end = ((a / ZONE + 1) * ZONE).min(b);
                if filtered && !zone_ok(&zones[a / ZONE]) {
                    a = end;
                    continue;
                }
                for i in a..end {
                    let s = dense[ent_name[i] as usize];
                    if s.flags & NF_OK == 0 {
                        continue;
                    }
                    let nh = NameHit { score: s.score, bits: s.bits, flags: s.flags, best: [0; 4] };
                    let m = memo.map_or(DirMemo::default(), |m| m[parent[i] as usize]);
                    if let Some(k) = self.score(i, nh, m, top.floor, &mut pbuf, re.as_ref()) {
                        top.push(k);
                    }
                }
                a = end;
            }
        })
    }

    fn selective(&self, lo: usize, hi: usize) -> Vec<Hit> {
        let idx = &self.live.base;
        let (ne_off, ne, parent) = (idx.name_ents_off(), idx.name_ents(), idx.parent());
        let ok: Vec<(u32, NameHit)> = self.names.iter().filter(|(_, h)| h.flags & NF_OK != 0).collect();
        top_k(ok.len(), self.q.limit, self.names.ok_entries, 0, |r, top| {
            let (mut pbuf, mut memo) = (Vec::new(), HashMap::<u32, DirMemo, crate::index::Fx>::default());
            let re = self.q.path_re.clone();
            for &(id, nh) in &ok[r] {
                for &e in &ne[ne_off[id as usize] as usize..ne_off[id as usize + 1] as usize] {
                    let i = e as usize;
                    if i < lo || i >= hi {
                        continue;
                    }
                    let m = if self.need_dirs { self.memo_of(parent[i], &mut memo, &mut |k| self.names.get(k)) } else { DirMemo::default() };
                    if let Some(k) = self.score(i, nh, m, top.floor, &mut pbuf, re.as_ref()) {
                        top.push(k);
                    }
                }
            }
        })
    }

    fn memo_of(&self, d: u32, cache: &mut HashMap<u32, DirMemo, crate::index::Fx>, hit: &mut impl FnMut(u32) -> Option<NameHit>) -> DirMemo {
        let idx = &self.live.base;
        let (de, en, dp) = (idx.dir_entry(), idx.ent_name(), idx.dir_parent());
        let mut chain = Vec::new();
        let mut k = d;
        let mut acc = loop {
            if k == 0 {
                break DirMemo::default();
            }
            if let Some(&m) = cache.get(&k) {
                break m;
            }
            chain.push(k);
            k = dp[k as usize];
        };
        for &k in chain.iter().rev() {
            acc = DirMemo::own(hit(en[de[k as usize] as usize])).under(acc);
            cache.insert(k, acc);
        }
        acc
    }
}

static MEMO_POOL: std::sync::Mutex<Vec<Vec<DirMemo>>> = std::sync::Mutex::new(Vec::new());

#[derive(PartialEq)]
struct NameKey {
    tokens: Vec<(Vec<u8>, Mode, bool)>,
    exts: Vec<Vec<u8>>,
    name_re: Option<String>,
    scope: Option<(usize, usize)>,
}

impl NameKey {
    fn of(q: &Query, scope: Option<(usize, usize)>) -> NameKey {
        NameKey {
            tokens: q.tokens.iter().map(|t| (t.text.clone(), t.mode, t.negate)).collect(),
            exts: q.exts.clone(),
            name_re: q.name_re.as_ref().map(|r| r.as_str().to_string()),
            scope,
        }
    }

    fn narrows(&self, prev: &NameKey) -> bool {
        self.exts == prev.exts
            && self.name_re == prev.name_re
            && self.scope == prev.scope
            && self.tokens.len() == prev.tokens.len()
            && self.tokens.iter().any(|t| !t.2)
            && self.tokens.iter().zip(&prev.tokens).all(|(a, b)| {
                a.1 == b.1
                    && a.2 == b.2
                    && if a.2 {
                        a.0 == b.0
                    } else if a.1 == Mode::Suffix {
                        a.0.ends_with(&b.0)
                    } else {
                        a.0.starts_with(&b.0) && takes_typos(&a.0, a.1) == takes_typos(&b.0, b.1)
                    }
            })
    }
}

pub struct Scored {
    at: std::time::Instant,
    key: NameKey,
    names: NameTable,
    memo: std::sync::OnceLock<Vec<DirMemo>>,
}

impl Drop for Scored {
    fn drop(&mut self) {
        if let Some(v) = self.memo.take() {
            let mut pool = MEMO_POOL.lock().unwrap();
            if pool.is_empty() {
                pool.push(v);
            }
        }
    }
}

/// Lives with the index it was scored against (`Live`).
#[derive(Default)]
pub struct NameCache {
    last: std::sync::Mutex<Option<std::sync::Arc<Scored>>>,
    #[allow(clippy::type_complexity)]
    scope: std::sync::Mutex<Option<((usize, usize), std::sync::Arc<NameSet>)>>,
}

impl NameCache {
    pub fn trim_if_idle(&self, idle: std::time::Duration) {
        let mut g = self.last.lock().unwrap();
        if g.as_ref().is_some_and(|s| s.at.elapsed() > idle) {
            *g = None;
            drop(g);
            *self.scope.lock().unwrap() = None;
            trim_pools();
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct DirMemo {
    bits: u32,
    best: [i16; 4],
}

impl DirMemo {
    #[inline]
    fn own(hit: Option<NameHit>) -> DirMemo {
        match hit {
            Some(h) if h.flags & NF_NEG != 0 => DirMemo { bits: u32::MAX, best: [0; 4] },
            Some(h) => DirMemo { bits: h.bits as u32, best: h.best },
            None => DirMemo::default(),
        }
    }

    #[inline]
    fn under(self, p: DirMemo) -> DirMemo {
        if p.bits == u32::MAX || self.bits == u32::MAX {
            return DirMemo { bits: u32::MAX, best: self.best };
        }
        let mut best = self.best;
        for (b, &q) in best.iter_mut().zip(&p.best) {
            *b = (*b).max(q);
        }
        DirMemo { bits: self.bits | p.bits, best }
    }
}

static HIT_POOL: std::sync::Mutex<Vec<Vec<NameHit>>> = std::sync::Mutex::new(Vec::new());

pub fn trim_pools() {
    BITS_POOL.lock().unwrap().clear();
    HIT_POOL.lock().unwrap().clear();
    DENSE_POOL.lock().unwrap().clear();
    MEMO_POOL.lock().unwrap().clear();
}

#[allow(clippy::type_complexity)]
static BITS_POOL: std::sync::Mutex<Vec<(Vec<u64>, Vec<u16>)>> = std::sync::Mutex::new(Vec::new());

static DENSE_POOL: std::sync::Mutex<Vec<Vec<Short>>> = std::sync::Mutex::new(Vec::new());

impl Drop for NameTable {
    fn drop(&mut self) {
        if !self.bits.is_empty() {
            for (chunk, hits) in self.bits.chunks_mut(CHUNK_WORDS).zip(&self.hits) {
                if !hits.is_empty() {
                    chunk.fill(0);
                }
            }
            let mut pool = BITS_POOL.lock().unwrap();
            if pool.is_empty() {
                pool.push((std::mem::take(&mut self.bits), std::mem::take(&mut self.rank)));
            }
        }
        if let Some(d) = self.dense.take() {
            let mut pool = DENSE_POOL.lock().unwrap();
            if pool.is_empty() {
                pool.push(d);
            }
        }
        let mut pool = HIT_POOL.lock().unwrap();
        if pool.is_empty() {
            *pool = std::mem::take(&mut self.hits);
            pool.iter_mut().for_each(Vec::clear);
        }
    }
}

struct NameTable {
    bits: Vec<u64>,
    rank: Vec<u16>,
    hits: Vec<Vec<NameHit>>,
    len: usize,
    ok_entries: usize,
    all: bool,
    dense: std::sync::OnceLock<Vec<Short>>,
}

#[derive(Clone, Copy, Default)]
struct Short {
    score: i16,
    bits: u8,
    flags: u8,
}

fn zeroed(n: usize) -> Vec<Short> {
    let layout = std::alloc::Layout::array::<Short>(n).expect("dense view size");
    if layout.size() == 0 {
        return Vec::new();
    }
    // Safety: allocated with Short's layout for `n`; all-zero bytes are a valid Short.
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *mut Short;
        if p.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Vec::from_raw_parts(p, n, n)
    }
}

impl NameTable {
    fn new(bits: Vec<u64>, rank: Vec<u16>, hits: Vec<Vec<NameHit>>, ok_entries: usize, all: bool) -> NameTable {
        let len = hits.iter().map(Vec::len).sum();
        NameTable { bits, rank, hits, len, ok_entries, all, dense: std::sync::OnceLock::new() }
    }

    fn dense(&self, idx: &Index) -> &[Short] {
        self.dense.get_or_init(|| {
            let spare = DENSE_POOL.lock().unwrap().pop().filter(|d| d.len() == idx.words * 64);
            let fresh = spare.is_none();
            let mut d = spare.unwrap_or_else(|| zeroed(idx.words * 64));
            let flags = [idx.bitmap(crate::index::BM_DOT), idx.bitmap(crate::index::BM_APP)];
            d.par_chunks_mut(CHUNK_WORDS * 64).enumerate().for_each(|(c, out)| {
                if !fresh {
                    out.fill(Short::default());
                }
                let base = c * CHUNK_WORDS * 64;
                if self.all {
                    for (j, o) in out.iter_mut().enumerate() {
                        let (w, i) = ((base + j) / 64, j % 64);
                        let f = |b: &[u64], nf: u8| if b[w] >> i & 1 != 0 { nf } else { 0 };
                        o.flags = NF_OK | f(flags[0], NF_DOT) | f(flags[1], NF_APP);
                    }
                    return;
                }
                let hits = self.hits[c].iter();
                let words = self.bits[c * CHUNK_WORDS..].iter().take(CHUNK_WORDS);
                let ids = words.enumerate().flat_map(|(wi, &b)| {
                    let mut b = b;
                    std::iter::from_fn(move || {
                        (b != 0).then(|| {
                            let j = wi * 64 + b.trailing_zeros() as usize;
                            b &= b - 1;
                            j
                        })
                    })
                });
                for (j, h) in ids.zip(hits) {
                    out[j] = Short { score: h.score, bits: h.bits, flags: h.flags };
                }
            });
            d
        })
    }

    #[inline(always)]
    fn get(&self, id: u32) -> Option<NameHit> {
        let (w, i) = (id as usize >> 6, id & 63);
        let b = self.bits[w];
        if b >> i & 1 == 0 {
            return None;
        }
        let r = self.rank[w] as usize + (b & ((1 << i) - 1)).count_ones() as usize;
        Some(self.hits[w / CHUNK_WORDS][r])
    }

    fn iter(&self) -> impl Iterator<Item = (u32, NameHit)> + '_ {
        self.hits.iter().enumerate().filter(|(_, hits)| !hits.is_empty()).flat_map(move |(c, hits)| {
            let words = self.bits[c * CHUNK_WORDS..].iter().take(CHUNK_WORDS);
            let ids = words.enumerate().flat_map(move |(wi, &b)| {
                let mut b = b;
                let base = ((c * CHUNK_WORDS + wi) * 64) as u32;
                std::iter::from_fn(move || {
                    (b != 0).then(|| {
                        let id = base + b.trailing_zeros();
                        b &= b - 1;
                        id
                    })
                })
            });
            ids.zip(hits.iter().copied())
        })
    }
}

struct NameSet {
    bits: Vec<u64>,
    todo: Vec<Todo>,
    words: usize,
    names: usize,
    busy: bool,
}

impl NameSet {
    fn new(bits: Vec<u64>) -> NameSet {
        let todo = bits.chunks(CHUNK_WORDS).map(nonzero).collect();
        let words = bits.iter().filter(|&&b| b != 0).count();
        let names = bits.iter().map(|b| b.count_ones() as usize).sum();
        NameSet { bits, todo, words, names, busy: false }
    }
}

type Todo = [u64; CHUNK_WORDS / 64];

fn nonzero(words: &[u64]) -> Todo {
    let mut out = Todo::default();
    for (o, g) in out.iter_mut().zip(words.chunks(64)) {
        *o = g.iter().enumerate().fold(0, |m, (b, &x)| m | ((x != 0) as u64) << b);
    }
    out
}

#[derive(Clone, Copy)]
struct NameHit {
    score: i16,
    bits: u8,
    flags: u8,
    best: [i16; 4],
}

const NF_OK: u8 = 1;
const NF_DOT: u8 = 2;
const NF_NEG: u8 = 8;
const NF_APP: u8 = 4;

fn name_flags(name: &[u8]) -> u8 {
    let mut f = 0;
    if name.first() == Some(&b'.') {
        f |= NF_DOT;
    }
    if name.ends_with(b".app") {
        f |= NF_APP;
    }
    f
}

#[inline]
fn rank_tweaks(flags: u8, kind: u8, mtime: u32, now: u32) -> i32 {
    let mut s = 0;
    if flags & NF_DOT != 0 {
        s -= 8;
    }
    if kind & FLAG_HIDDEN != 0 {
        s -= 8;
    }
    // Apps are dirs, or symlinks into the cryptex (/Applications/Safari.app).
    if matches!(kind & 3, KIND_DIR | KIND_LINK) && flags & NF_APP != 0 {
        s += 25;
    }
    let age = now.saturating_sub(mtime);
    s += match age {
        0..=86_400 => 10,
        86_401..=604_800 => 7,
        604_801..=2_592_000 => 4,
        2_592_001..=31_536_000 => 1,
        _ => 0,
    };
    s
}
