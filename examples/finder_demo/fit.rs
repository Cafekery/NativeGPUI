use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use gpui::{
    Context, Font, FontWeight, Pixels, ShapedLine, SharedString, TextRun, WindowTextSystem, hsla,
};
use native_gpui::tokens;

const SLACK: f32 = 0.5;
const TABLE: u128 = 1 << 14;
const BATCH: usize = 32;

fn shape(texts: &WindowTextSystem, text: SharedString, font: &Font, size: Pixels) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: hsla(0., 0., 0., 1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    texts.shape_line(text, size, &[run], None)
}

pub(crate) fn line_width(texts: &WindowTextSystem, text: SharedString, size: Pixels) -> f32 {
    f32::from(shape(texts, text, &tokens::system_font(FontWeight::NORMAL), size).width)
}

struct Advances {
    ascii: Vec<f32>,
    rest: HashMap<u128, f32>,
}

impl Default for Advances {
    fn default() -> Self {
        Self {
            ascii: vec![f32::NAN; TABLE as usize],
            rest: HashMap::new(),
        }
    }
}

impl Advances {
    fn estimate(&self, text: &str) -> Option<f32> {
        keys(text).try_fold(0., |sum, key| {
            let advance = match key? {
                key if key < TABLE => Some(self.ascii[key as usize]).filter(|a| !a.is_nan()),
                key => self.rest.get(&key).copied(),
            };
            Some(sum + advance?)
        })
    }

    fn learn(&mut self, line: &ShapedLine) {
        let glyphs = || line.runs.iter().flat_map(|run| &run.glyphs);
        let starts = line.text.char_indices().map(|(at, _)| at);
        if glyphs().map(|glyph| glyph.index).ne(starts) {
            return;
        }
        let ends = glyphs().skip(1).map(|glyph| glyph.position.x);
        for ((key, glyph), end) in keys(&line.text).zip(glyphs()).zip(ends.chain([line.width])) {
            let advance = f32::from(end - glyph.position.x);
            match key {
                Some(key) if key < TABLE => self.ascii[key as usize] = advance,
                Some(key) => _ = self.rest.insert(key, advance),
                None => {}
            }
        }
    }
}

fn keys(text: &str) -> impl Iterator<Item = Option<u128>> + '_ {
    let mut chars = text.chars().chain(['\0', '\0']);
    let (mut a, mut b, mut c) = (
        '\0',
        chars.next().unwrap_or('\0'),
        chars.next().unwrap_or('\0'),
    );
    std::iter::from_fn(move || {
        if b == '\0' {
            return None;
        }
        let d = chars.next().unwrap_or('\0');
        // The system font's colon takes a contextual alternate between figures or capitals.
        let [w, x, y, z] = [
            if b == ':' { a } else { '\0' },
            b,
            c,
            if c == ':' { d } else { '\0' },
        ]
        .map(u128::from);
        let key = match w | z == 0 && x < 128 && y < 128 {
            true => x << 7 | y,
            false => w << 63 | x << 42 | y << 21 | z,
        };
        let plain = plain(b);
        (a, b, c) = (b, c, d);
        Some(plain.then_some(key))
    })
}

fn plain(c: char) -> bool {
    // A soft hyphen's width depends on both its neighbours.
    matches!(
        c,
        ' '..='~' | '\u{a0}'..='\u{ac}' | '\u{ae}'..='\u{17f}' | '\u{2010}'..='\u{2027}' | '\u{202f}'
    )
}

static ADVANCES: LazyLock<Mutex<HashMap<(Font, u32), Advances>>> = LazyLock::new(Mutex::default);

pub(crate) fn widest<T>(
    texts: &WindowTextSystem,
    font: &Font,
    size: Pixels,
    items: &[T],
    text: impl Fn(&T) -> SharedString,
    total: impl Fn(&T, f32) -> f32,
) -> f32 {
    let mut all = ADVANCES.lock().unwrap();
    let advances = all
        .entry((font.clone(), f32::from(size).to_bits()))
        .or_default();
    let mut best = 0_f32;
    let mut ranked = Vec::with_capacity(items.len());
    let mut rank =
        |item, text, width: f32| ranked.push((total(item, width + SLACK), width, text, item));
    let mut unknown = Vec::new();
    for (at, item) in items.iter().enumerate() {
        let text = text(item);
        match advances.estimate(&text) {
            Some(width) => rank(item, text, width),
            None => unknown.push((item, text)),
        }
        if unknown.len() < BATCH && at + 1 < items.len() {
            continue;
        }
        let batch: Vec<&str> = unknown
            .iter()
            .map(|(_, text)| text.as_ref())
            .filter(|text| text.chars().all(plain))
            .collect();
        if !batch.is_empty() {
            advances.learn(&shape(texts, batch.join("\u{2003}").into(), font, size));
        }
        for (item, text) in unknown.drain(..) {
            match advances.estimate(&text) {
                Some(width) => rank(item, text, width),
                None => {
                    let line = shape(texts, text, font, size);
                    best = best.max(total(item, f32::from(line.width)));
                    advances.learn(&line);
                }
            }
        }
    }
    ranked.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    let mut trusted = true;
    let mut shaped = HashMap::new();
    for (bound, estimate, text, item) in ranked {
        if trusted && bound <= best {
            break;
        }
        let width = *shaped
            .entry(text.clone())
            .or_insert_with(|| f32::from(shape(texts, text, font, size).width));
        trusted &= (width - estimate).abs() <= SLACK;
        best = best.max(total(item, width));
    }
    best
}

pub(crate) fn in_background<V: 'static>(
    cx: &mut Context<V>,
    measure: impl FnOnce(&WindowTextSystem) -> f32 + Send + 'static,
    apply: impl FnOnce(&mut V, f32, &mut Context<V>) + 'static,
) {
    let texts = cx.text_system().clone();
    cx.spawn(async move |this, cx| {
        let widest = cx
            .background_executor()
            .spawn(async move { measure(&WindowTextSystem::new(texts)) })
            .await;
        this.update(cx, |this, cx| apply(this, widest, cx))
    })
    .detach();
}
