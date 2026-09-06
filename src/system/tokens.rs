//! The system's own colours, fonts, symbols and file-type icons, for the parts gpui draws itself.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use block2::StackBlock;
use gpui::{
    App, AsyncApp, Font, FontFeatures, FontStyle, FontWeight, Global, Hsla, Img, Pixels,
    RenderImage, Rgba, Size, Styled, img, px, size,
};
use image::{Frame, RgbaImage};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceName, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSAttributedStringNSStringDrawing, NSBitmapFormat, NSBitmapImageRep, NSColor,
    NSDeviceRGBColorSpace, NSFont, NSFontAttributeName, NSFontWeight, NSFontWeightBlack,
    NSFontWeightBold, NSFontWeightHeavy, NSFontWeightLight, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSFontWeightThin, NSFontWeightUltraLight,
    NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSImageSymbolScale,
    NSKernAttributeName, NSLigatureAttributeName, NSScreen, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSDictionary, NSDistributedNotificationCenter,
    NSKeyValueObservingOptions, NSNotification, NSNotificationCenter, NSNumber,
    NSObjectNSKeyValueObserverRegistration, NSPoint, NSRect, NSSize, NSString, ns_string,
};

use crate::bridge::host::ControlSize;
use crate::system::color::from_nscolor;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    fn ns_name(self) -> &'static NSAppearanceName {
        match self {
            Appearance::Light => unsafe { NSAppearanceNameAqua },
            Appearance::Dark => unsafe { NSAppearanceNameDarkAqua },
        }
    }

    pub fn ns_appearance(self) -> Retained<NSAppearance> {
        NSAppearance::appearanceNamed(self.ns_name())
            .expect("the two base appearances always resolve")
    }

    /// Collapse any appearance onto one of the two worlds.
    pub fn of(appearance: &NSAppearance) -> Appearance {
        let names = NSArray::from_slice(&[Appearance::Light.ns_name(), Appearance::Dark.ns_name()]);
        match appearance.bestMatchFromAppearancesWithNames(&names) {
            Some(name) if &*name == unsafe { NSAppearanceNameDarkAqua } => Appearance::Dark,
            Some(_) => Appearance::Light,
            // For an appearance Apple has not shipped yet: only dark and vibrant ones vibrate.
            None if appearance.allowsVibrancy() => Appearance::Dark,
            None => Appearance::Light,
        }
    }
}

thread_local! {
    static APPEARANCE_MEMO: Cell<(*const NSAppearance, Appearance)> =
        const { Cell::new((ptr::null(), Appearance::Light)) };
}

pub fn current_appearance() -> Appearance {
    let Some(mtm) = MainThreadMarker::new() else {
        return Appearance::Light;
    };
    let effective = NSApplication::sharedApplication(mtm).effectiveAppearance();
    let key = Retained::as_ptr(&effective);
    APPEARANCE_MEMO.with(|memo| {
        let (cached_key, cached) = memo.get();
        if cached_key == key {
            return cached;
        }
        let resolved = Appearance::of(&effective);
        memo.set((key, resolved));
        resolved
    })
}

/// Force this process's appearance, or `None` to follow the system again.
pub fn set_app_appearance(appearance: Option<Appearance>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let value = appearance.map(|a| a.ns_appearance());
    app.setAppearance(value.as_deref());
}

type ColorFn = fn() -> Retained<NSColor>;

macro_rules! system_colors {
    ($(($variant:ident, $objc:ident)),* $(,)?) => {
        /// A semantic macOS colour: a role, not a value.
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
        #[non_exhaustive]
        pub enum SystemColor {
            $($variant),*
        }

        impl SystemColor {
            pub const ALL: &'static [SystemColor] = &[$(SystemColor::$variant),*];

            pub const fn objc_name(self) -> &'static str {
                match self {
                    $(SystemColor::$variant => stringify!($objc)),*
                }
            }

            const fn getter(self) -> ColorFn {
                match self {
                    $(SystemColor::$variant => NSColor::$objc),*
                }
            }
        }
    };
}

system_colors![
    (Label, labelColor),
    (SecondaryLabel, secondaryLabelColor),
    (TertiaryLabel, tertiaryLabelColor),
    (QuaternaryLabel, quaternaryLabelColor),
    (QuinaryLabel, quinaryLabelColor),
    (Text, textColor),
    (PlaceholderText, placeholderTextColor),
    (HeaderText, headerTextColor),
    (TextBackground, textBackgroundColor),
    (SelectedText, selectedTextColor),
    (SelectedTextBackground, selectedTextBackgroundColor),
    (
        UnemphasizedSelectedTextBackground,
        unemphasizedSelectedTextBackgroundColor
    ),
    (TextInsertionPoint, textInsertionPointColor),
    (WindowBackground, windowBackgroundColor),
    (UnderPageBackground, underPageBackgroundColor),
    (ControlBackground, controlBackgroundColor),
    (Control, controlColor),
    (ControlText, controlTextColor),
    (DisabledControlText, disabledControlTextColor),
    (SelectedControl, selectedControlColor),
    (SelectedControlText, selectedControlTextColor),
    (
        AlternateSelectedControlText,
        alternateSelectedControlTextColor
    ),
    (SelectedContentBackground, selectedContentBackgroundColor),
    (
        UnemphasizedSelectedContentBackground,
        unemphasizedSelectedContentBackgroundColor
    ),
    (Separator, separatorColor),
    (Grid, gridColor),
    (Link, linkColor),
    (ControlAccent, controlAccentColor),
    (KeyboardFocusIndicator, keyboardFocusIndicatorColor),
    (FindHighlight, findHighlightColor),
    (Shadow, shadowColor),
    (SystemFill, systemFillColor),
    (SecondarySystemFill, secondarySystemFillColor),
    (TertiarySystemFill, tertiarySystemFillColor),
    (QuaternarySystemFill, quaternarySystemFillColor),
    (QuinarySystemFill, quinarySystemFillColor),
    (SystemRed, systemRedColor),
    (SystemOrange, systemOrangeColor),
    (SystemYellow, systemYellowColor),
    (SystemGreen, systemGreenColor),
    (SystemMint, systemMintColor),
    (SystemTeal, systemTealColor),
    (SystemCyan, systemCyanColor),
    (SystemBlue, systemBlueColor),
    (SystemIndigo, systemIndigoColor),
    (SystemPurple, systemPurpleColor),
    (SystemPink, systemPinkColor),
    (SystemBrown, systemBrownColor),
    (SystemGray, systemGrayColor),
];

impl SystemColor {
    pub fn ns_color(self) -> Retained<NSColor> {
        (self.getter())()
    }

    /// Resolve for an explicit appearance, uncached.
    pub fn try_resolve(self, appearance: Appearance) -> Option<Hsla> {
        let getter = self.getter();
        with_appearance(appearance, || from_nscolor(&getter()))
    }
}

fn with_appearance<T>(appearance: Appearance, f: impl Fn() -> T) -> T {
    let slot: RefCell<Option<T>> = RefCell::new(None);
    let block = StackBlock::new(|| *slot.borrow_mut() = Some(f()));
    appearance
        .ns_appearance()
        .performAsCurrentDrawingAppearance(&block);
    slot.into_inner()
        .expect("performAsCurrentDrawingAppearance never invoked the block")
}

/// Convert any `NSColor` to a gpui [`Hsla`], `None` if it refuses sRGB conversion.
pub fn srgb(color: &NSColor) -> Option<Hsla> {
    from_nscolor(color)
}

// `labelColor`'s own value, so a colour that refuses conversion degrades to readable text.
fn refusal_fallback(appearance: Appearance) -> Hsla {
    let luminance = match appearance {
        Appearance::Light => 0.0,
        Appearance::Dark => 1.0,
    };
    Rgba {
        r: luminance,
        g: luminance,
        b: luminance,
        a: 0.847,
    }
    .into()
}

/// Which colours refuse sRGB conversion under `appearance`.
pub fn refused(appearance: Appearance) -> Vec<SystemColor> {
    SystemColor::ALL
        .iter()
        .copied()
        .filter(|c| c.try_resolve(appearance).is_none())
        .collect()
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[derive(Default)]
struct Cache {
    generation: u64,
    palettes: [Option<Vec<Option<Hsla>>>; 2],
    scale: Option<f32>,
    symbols: HashMap<SymbolKey, Option<Symbol>>,
    file_icons: HashMap<FileIconKey, Option<Symbol>>,
    palette_resolves: u64,
    symbol_rasters: u64,
    file_icon_rasters: u64,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CacheStats {
    pub palette_resolves: u64,
    pub symbol_rasters: u64,
    pub file_icon_rasters: u64,
    pub generation: u64,
}

pub fn cache_stats() -> CacheStats {
    CACHE.with(|cache| {
        let cache = cache.borrow();
        CacheStats {
            palette_resolves: cache.palette_resolves,
            symbol_rasters: cache.symbol_rasters,
            file_icon_rasters: cache.file_icon_rasters,
            generation: cache.generation,
        }
    })
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

fn with_cache<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let generation = GENERATION.load(Ordering::Acquire);
        if cache.generation != generation {
            cache.generation = generation;
            cache.palettes = [None, None];
            cache.scale = None;
            cache.symbols.clear();
        }
        f(&mut cache)
    })
}

pub fn system_color(color: SystemColor) -> Hsla {
    let appearance = current_appearance();
    with_cache(|cache| {
        let slot = &mut cache.palettes[appearance as usize];
        let palette = match slot {
            Some(palette) => palette,
            none => {
                cache.palette_resolves += 1;
                none.insert(with_appearance(appearance, || {
                    SystemColor::ALL
                        .iter()
                        .map(|c| from_nscolor(&c.ns_color()))
                        .collect::<Vec<_>>()
                }))
            }
        };
        palette[color as usize].unwrap_or_else(|| refusal_fallback(appearance))
    })
}

pub fn accent() -> Hsla {
    system_color(SystemColor::ControlAccent)
}

/// The one background colour every opaque surface of a window resolves to.
pub const WINDOW_BACKGROUND: SystemColor = SystemColor::WindowBackground;

pub fn window_background() -> Hsla {
    system_color(WINDOW_BACKGROUND)
}

// `NSScreen` answers only on the main thread; a worker draws at the scale it last gave.
static SCALE: AtomicU32 = AtomicU32::new(2.0f32.to_bits());

fn display_scale() -> f32 {
    with_cache(|cache| {
        *cache.scale.get_or_insert_with(|| {
            let Some(mtm) = MainThreadMarker::new() else {
                return f32::from_bits(SCALE.load(Ordering::Acquire));
            };
            let scale = NSScreen::screens(mtm)
                .iter()
                .map(|s| s.backingScaleFactor() as f32)
                .fold(1.0f32, f32::max);
            SCALE.store(scale.to_bits(), Ordering::Release);
            scale
        })
    })
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct SystemTheme {
    pub appearance: Appearance,
    pub accent: Hsla,
    pub generation: u64,
}

impl Global for SystemTheme {}

fn snapshot() -> SystemTheme {
    SystemTheme {
        appearance: current_appearance(),
        accent: accent(),
        generation: GENERATION.load(Ordering::Acquire),
    }
}

// An accent-only change posts `AppleColorPreferencesChanged` and never reaches the KVO.
const DISTRIBUTED_NOTIFICATIONS: &[&str] = &[
    "AppleColorPreferencesChangedNotification",
    "AppleInterfaceThemeChangedNotification",
    "AppleAquaColorVariantChanged",
    "AppleInterfaceStyleChangedNotification",
];

const LOCAL_NOTIFICATIONS: &[&str] = &[
    "NSSystemColorsDidChangeNotification",
    "NSApplicationDidChangeScreenParametersNotification",
];

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGThemeObserver"]
    #[ivars = ThemeObserverIvars]
    struct ThemeObserver;

    unsafe impl NSObjectProtocol for ThemeObserver {}

    impl ThemeObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: *mut AnyObject,
            _change: *mut AnyObject,
            _context: *mut std::ffi::c_void,
        ) {
            bump(&self.ivars().cx);
        }
    }
);

struct ThemeObserverIvars {
    cx: AsyncApp,
}

fn bump(cx: &AsyncApp) {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    APPEARANCE_MEMO.with(|memo| memo.set((ptr::null(), Appearance::Light)));
    // Deferred: a gpui-sourced appearance change arrives while gpui still holds its `App` borrow.
    let cx2 = cx.clone();
    cx.spawn(async move |_| {
        cx2.update(|app| app.set_global(snapshot()));
    })
    .detach();
}

struct Observers {
    _tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    _kvo: Retained<ThemeObserver>,
}

thread_local! {
    static OBSERVERS: RefCell<Option<Observers>> = const { RefCell::new(None) };
}

/// Start tracking the system and publish the first [`SystemTheme`]. Idempotent.
pub fn init(cx: &mut App) {
    cx.set_global(snapshot());

    if OBSERVERS.with(|slot| slot.borrow().is_some()) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let async_cx = cx.to_async();
    let mut tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>> = Vec::new();

    let distributed = NSDistributedNotificationCenter::defaultCenter();
    for name in DISTRIBUTED_NOTIFICATIONS {
        let name = NSString::from_str(name);
        let cx = async_cx.clone();
        let block = StackBlock::new(move |_: ptr::NonNull<NSNotification>| bump(&cx)).copy();
        tokens.push(unsafe {
            distributed.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
        });
    }

    let local = NSNotificationCenter::defaultCenter();
    for name in LOCAL_NOTIFICATIONS {
        let name = NSString::from_str(name);
        let cx = async_cx.clone();
        let block = StackBlock::new(move |_: ptr::NonNull<NSNotification>| bump(&cx)).copy();
        tokens.push(unsafe {
            local.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
        });
    }

    let kvo = ThemeObserver::alloc(mtm).set_ivars(ThemeObserverIvars {
        cx: async_cx.clone(),
    });
    let kvo: Retained<ThemeObserver> = unsafe { msg_send![super(kvo), init] };
    let nsapp = NSApplication::sharedApplication(mtm);
    let app: &NSObject = nsapp.as_ref();
    unsafe {
        app.addObserver_forKeyPath_options_context(
            kvo.as_ref(),
            ns_string!("effectiveAppearance"),
            NSKeyValueObservingOptions::New,
            ptr::null_mut(),
        );
    }

    OBSERVERS.with(|slot| {
        *slot.borrow_mut() = Some(Observers {
            _tokens: tokens,
            _kvo: kvo,
        })
    });
}

pub const fn system_font_family() -> &'static str {
    ".AppleSystemUIFont"
}

pub const fn text_font_family_name() -> &'static str {
    "SF Pro Text"
}

/// `SF Pro Text` if it is actually installed — a developer download, not part of stock macOS.
pub fn text_font_family(cx: &App) -> Option<&'static str> {
    let text_system = cx.text_system();
    let resolve = |family: &str| {
        let mut font = system_font(FontWeight::NORMAL);
        font.family = family.to_string().into();
        text_system.resolve_font(&font)
    };
    // gpui resolves an unknown family to a silent fallback, so two nonexistent names pin its id.
    let fallback = resolve("NG-No-Such-Family-9x7");
    if fallback != resolve("NG-Also-Not-Installed-4b2") {
        return None;
    }
    (resolve(text_font_family_name()) != fallback).then(text_font_family_name)
}

pub fn system_font_size() -> Pixels {
    px(NSFont::systemFontSize() as f32)
}

pub fn small_system_font_size() -> Pixels {
    px(NSFont::smallSystemFontSize() as f32)
}

/// The font size that belongs to a [`ControlSize`].
pub fn system_font_size_for(size: ControlSize) -> Pixels {
    px(NSFont::systemFontSizeForControlSize(size.into()) as f32)
}

pub fn label_font_size() -> Pixels {
    px(NSFont::labelFontSize() as f32)
}

pub fn system_font(weight: FontWeight) -> Font {
    Font {
        family: system_font_family().into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

pub fn system_font_tabular(weight: FontWeight) -> Font {
    Font {
        features: FontFeatures(Arc::new(vec![("tnum".to_string(), 1)])),
        ..system_font(weight)
    }
}

pub fn ns_font_weight(weight: FontWeight) -> NSFontWeight {
    let table: [(f32, NSFontWeight); 9] = unsafe {
        [
            (100.0, NSFontWeightUltraLight),
            (200.0, NSFontWeightThin),
            (300.0, NSFontWeightLight),
            (400.0, NSFontWeightRegular),
            (500.0, NSFontWeightMedium),
            (600.0, NSFontWeightSemibold),
            (700.0, NSFontWeightBold),
            (800.0, NSFontWeightHeavy),
            (900.0, NSFontWeightBlack),
        ]
    };
    table
        .iter()
        .min_by(|a, b| (a.0 - weight.0).abs().total_cmp(&(b.0 - weight.0).abs()))
        .map(|(_, w)| *w)
        .expect("weight table is never empty")
}

#[derive(Clone, PartialEq, Debug)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub resolved_name: String,
}

pub fn system_font_metrics(size: Pixels, weight: FontWeight) -> FontMetrics {
    let font = NSFont::systemFontOfSize_weight(f32::from(size) as f64, ns_font_weight(weight));
    FontMetrics {
        ascent: font.ascender() as f32,
        descent: font.descender() as f32,
        leading: font.leading() as f32,
        cap_height: font.capHeight() as f32,
        x_height: font.xHeight() as f32,
        resolved_name: font.fontName().to_string(),
    }
}

pub fn ns_font(family: &str, size: Pixels, weight: FontWeight) -> Option<Retained<NSFont>> {
    let points = f32::from(size) as f64;
    if family == system_font_family() {
        Some(NSFont::systemFontOfSize_weight(
            points,
            ns_font_weight(weight),
        ))
    } else {
        NSFont::fontWithName_size(&NSString::from_str(family), points)
    }
}

fn typeset_width(font: &NSFont, text: &str) -> f64 {
    let zero = NSNumber::new_i32(0);
    let font_obj: &AnyObject = font.as_ref();
    let zero_obj: &AnyObject = zero.as_ref();
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &[
            unsafe { NSFontAttributeName },
            unsafe { NSKernAttributeName },
            unsafe { NSLigatureAttributeName },
        ],
        &[font_obj, zero_obj, zero_obj],
    );
    let string = NSString::from_str(text);
    let attributed = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &string,
            Some(&attrs),
        )
    };
    attributed.size().width
}

pub fn appkit_string_width(
    family: &str,
    size: Pixels,
    weight: FontWeight,
    text: &str,
) -> Option<f32> {
    Some(typeset_width(&*ns_font(family, size, weight)?, text) as f32)
}

// Differential — `width(ch ch) - width(ch)` — to cancel the string's own side bearings.
pub fn appkit_advance(family: &str, size: Pixels, weight: FontWeight, ch: char) -> Option<f32> {
    let font = ns_font(family, size, weight)?;
    Some(
        (typeset_width(&font, &format!("{ch}{ch}")) - typeset_width(&font, &ch.to_string())) as f32,
    )
}

pub fn gpui_advance(
    cx: &App,
    family: &str,
    size: Pixels,
    weight: FontWeight,
    ch: char,
) -> Option<f32> {
    let text_system = cx.text_system();
    let mut font = system_font(weight);
    font.family = family.to_string().into();
    let id = text_system.resolve_font(&font);
    Some(f32::from(text_system.advance(id, size, ch).ok()?.width))
}

/// How far gpui's advance for `ch` is from AppKit's, in points; positive means gpui is wider.
pub fn advance_delta(
    cx: &App,
    family: &str,
    size: Pixels,
    weight: FontWeight,
    ch: char,
) -> Option<f32> {
    Some(gpui_advance(cx, family, size, weight, ch)? - appkit_advance(family, size, weight, ch)?)
}

/// How a symbol's layers get their colour.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SymbolColor {
    Label,
    Hierarchical(SystemColor),
    /// One colour per layer, in order; layers past the end of the list keep the last colour.
    Palette(Vec<SystemColor>),
    Multicolor,
    /// No colour configuration at all: a template symbol comes out black.
    Raw,
}

impl SymbolColor {
    fn ns_configuration(&self) -> Option<Retained<NSImageSymbolConfiguration>> {
        match self {
            SymbolColor::Raw => None,
            SymbolColor::Label => Some(
                NSImageSymbolConfiguration::configurationWithHierarchicalColor(
                    &SystemColor::Label.ns_color(),
                ),
            ),
            SymbolColor::Hierarchical(c) => {
                Some(NSImageSymbolConfiguration::configurationWithHierarchicalColor(&c.ns_color()))
            }
            SymbolColor::Palette(colors) => {
                let retained: Vec<Retained<NSColor>> =
                    colors.iter().map(|c| c.ns_color()).collect();
                let refs: Vec<&NSColor> = retained.iter().map(|c| &**c).collect();
                Some(NSImageSymbolConfiguration::configurationWithPaletteColors(
                    &NSArray::from_slice(&refs),
                ))
            }
            SymbolColor::Multicolor => {
                Some(NSImageSymbolConfiguration::configurationPreferringMulticolor())
            }
        }
    }
}

/// A rasterised system image, ready to hand to gpui.
#[derive(Clone)]
pub struct Symbol {
    pub image: Arc<RenderImage>,
    /// Logical size in points; the buffer is `scale` times this.
    pub size: Size<Pixels>,
    pub scale: f32,
}

impl Symbol {
    pub fn img(&self) -> Img {
        img(self.image.clone())
            .w(self.size.width)
            .h(self.size.height)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SymbolKey {
    name: String,
    size: u32,
    weight: u32,
    color: SymbolColor,
    appearance: Appearance,
}

/// Render an SF Symbol for gpui, in `labelColor` at the current appearance.
pub fn sf_symbol(name: &str, size: Pixels, weight: FontWeight) -> Option<Symbol> {
    sf_symbol_colored(name, size, weight, &SymbolColor::Label)
}

/// [`sf_symbol`] with an explicit colour configuration.
pub fn sf_symbol_colored(
    name: &str,
    size: Pixels,
    weight: FontWeight,
    color: &SymbolColor,
) -> Option<Symbol> {
    let appearance = current_appearance();
    let scale = display_scale();
    let key = SymbolKey {
        name: name.to_string(),
        size: f32::from(size).to_bits(),
        weight: weight.0.to_bits(),
        color: color.clone(),
        appearance,
    };
    with_cache(|cache| match cache.symbols.get(&key) {
        Some(hit) => hit.clone(),
        None => {
            cache.symbol_rasters += 1;
            let symbol = rasterize_symbol(name, size, weight, color, appearance, scale);
            cache.symbols.insert(key, symbol.clone());
            symbol
        }
    })
}

fn rasterize_symbol(
    name: &str,
    size: Pixels,
    weight: FontWeight,
    color: &SymbolColor,
    appearance: Appearance,
    scale: f32,
) -> Option<Symbol> {
    let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;

    let sized = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        f32::from(size) as f64,
        ns_font_weight(weight),
        NSImageSymbolScale::Medium,
    );
    let configuration = match color.ns_configuration() {
        Some(c) => sized.configurationByApplyingConfiguration(&c),
        None => sized,
    };
    let image = base.imageWithSymbolConfiguration(&configuration)?;

    let points = image.size();
    let w = ((points.width * scale as f64).round() as usize).max(1);
    let h = ((points.height * scale as f64).round() as usize).max(1);

    // A semantic tint resolves at raster time, picking up the ambient appearance if none is set.
    let rgba = with_appearance(appearance, || draw(&image, points, w, h))?;

    Some(Symbol {
        image: Arc::new(RenderImage::new(vec![Frame::new(
            RgbaImage::from_raw(w as u32, h as u32, to_bgra_straight(&rgba))
                .expect("raster buffer is w*h*4 by construction"),
        )])),
        size: size_of_points(points),
        scale,
    })
}

/// Which system icon to fetch.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FileType<'a> {
    Folder,
    /// A filename extension without the dot, e.g. `"zip"`.
    Extension(&'a str),
    /// A uniform type identifier, e.g. `"public.plain-text"`.
    Uti(&'a str),
    /// This file's own icon, e.g. a volume's drive artwork, which its type alone does not give.
    Path(&'a str),
}

impl FileType<'_> {
    fn tag(self) -> String {
        match self {
            FileType::Folder => "uti:public.folder".into(),
            FileType::Uti(uti) => format!("uti:{uti}"),
            FileType::Extension(extension) => format!("ext:{extension}"),
            FileType::Path(path) => format!("path:{path}"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FileIconKey {
    tag: String,
    size: u32,
    scale: u32,
}

/// The system's own artwork for a file type, rasterised for gpui.
pub fn file_icon(kind: FileType<'_>, size: Pixels) -> Option<Symbol> {
    let scale = display_scale();
    let key = FileIconKey {
        tag: kind.tag(),
        size: f32::from(size).to_bits(),
        scale: scale.to_bits(),
    };
    with_cache(|cache| match cache.file_icons.get(&key) {
        Some(hit) => hit.clone(),
        None => {
            cache.file_icon_rasters += 1;
            let icon = rasterize_file_icon(kind, size, scale);
            cache.file_icons.insert(key, icon.clone());
            icon
        }
    })
}

pub fn folder_icon(size: Pixels) -> Option<Symbol> {
    file_icon(FileType::Folder, size)
}

unsafe extern "C" {
    fn getxattr(
        path: *const std::os::raw::c_char,
        name: *const std::os::raw::c_char,
        value: *mut std::os::raw::c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> isize;
}

// FinderInfo's `finderFlags` sit at byte 8, big-endian; `kHasCustomIcon` is bit 0x0400 of it,
// the same offset for files, folders and (as `kExtFinderInfo`) volumes.
const HAS_CUSTOM_ICON: u16 = 0x0400;

/// Whether `path` carries its own Finder icon (an app's own artwork aside — that comes from its
/// bundle, not this bit). Cheap: one `getxattr`, no icon work.
pub fn has_custom_icon(path: &std::path::Path) -> bool {
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    let name = c"com.apple.FinderInfo";
    let mut info = [0u8; 32];
    let read = unsafe {
        getxattr(
            path.as_ptr(),
            name.as_ptr(),
            info.as_mut_ptr().cast(),
            info.len(),
            0,
            0,
        )
    };
    read >= 10 && u16::from_be_bytes([info[8], info[9]]) & HAS_CUSTOM_ICON != 0
}

// `typeWithFilenameExtension:` answers a dynamic type for `iconset` and the legacy one for `app`.
// `UTType` only answers `AnyClass::get` once its framework is in the process, and nothing else
// here loads it on macOS 26.
#[link(name = "UniformTypeIdentifiers", kind = "framework")]
unsafe extern "C" {}

fn ut_type(extension: bool, text: &str) -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"UTType")?;
    let text = NSString::from_str(text);
    if !extension {
        return unsafe { msg_send![class, typeWithIdentifier: &*text] };
    }
    let candidates: Retained<NSArray<AnyObject>> = unsafe {
        msg_send![
            class,
            typesWithTag: &*text,
            tagClass: ns_string!("public.filename-extension"),
            conformingToType: ptr::null::<AnyObject>(),
        ]
    };
    for candidate in candidates.iter() {
        let declared: bool = unsafe { msg_send![&*candidate, isDeclared] };
        if declared {
            return Some(candidate);
        }
    }
    unsafe { msg_send![class, typeWithFilenameExtension: &*text] }
}

fn rasterize_file_icon(kind: FileType<'_>, size: Pixels, scale: f32) -> Option<Symbol> {
    let workspace = NSWorkspace::sharedWorkspace();
    let image: Option<Retained<NSImage>> = match kind {
        FileType::Path(path) => {
            let path = NSString::from_str(path);
            unsafe { msg_send![&*workspace, iconForFile: &*path] }
        }
        FileType::Folder => {
            let content = ut_type(false, "public.folder")?;
            unsafe { msg_send![&*workspace, iconForContentType: &*content] }
        }
        FileType::Uti(uti) => {
            let content = ut_type(false, uti)?;
            unsafe { msg_send![&*workspace, iconForContentType: &*content] }
        }
        FileType::Extension(extension) => {
            let content = ut_type(true, extension)?;
            unsafe { msg_send![&*workspace, iconForContentType: &*content] }
        }
    };
    let image = image?;

    // `iconForContentType:` always reports 32x32 points; `drawInRect:` picks by destination pixels.
    let points = NSSize::new(f32::from(size) as f64, f32::from(size) as f64);
    let w = ((points.width * scale as f64).round() as usize).max(1);
    let h = ((points.height * scale as f64).round() as usize).max(1);

    let rgba = draw(&image, points, w, h)?;

    Some(Symbol {
        image: Arc::new(RenderImage::new(vec![Frame::new(
            RgbaImage::from_raw(w as u32, h as u32, to_bgra_straight(&rgba))
                .expect("raster buffer is w*h*4 by construction"),
        )])),
        size: size_of_points(points),
        scale,
    })
}

fn size_of_points(points: NSSize) -> Size<Pixels> {
    size(px(points.width as f32), px(points.height as f32))
}

pub(crate) fn draw(image: &NSImage, points: NSSize, w: usize, h: usize) -> Option<Vec<u8>> {
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            w as isize,
            h as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            (w * 4) as isize,
            32,
        )
    }?;
    // The rep's size is in points while its buffer is in pixels; that mismatch is the 2x transform.
    rep.setSize(points);

    // A bitmap rep with no current graphics context accepts `drawInRect` and produces zeroes.
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), points));
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let src = rep.bitmapData();
    if src.is_null() {
        return None;
    }
    let stride = rep.bytesPerRow() as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        unsafe {
            ptr::copy_nonoverlapping(src.add(y * stride), out.as_mut_ptr().add(y * w * 4), w * 4);
        }
    }
    Some(out)
}

// gpui's `RenderImage` wants straight-alpha BGRA, so swap R/B *and* divide out alpha.
pub(crate) fn to_bgra_straight(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dominant_hue(bgra: &[u8]) -> (Option<f32>, f32) {
        let mut bins = [0usize; 36];
        let mut achromatic = 0usize;
        let mut covered = 0usize;
        for p in bgra.chunks_exact(4) {
            if p[3] < 200 {
                continue;
            }
            covered += 1;
            let (b, g, r) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let chroma = max - min;
            if chroma < 24.0 {
                achromatic += 1;
                continue;
            }
            let hue = if max == r {
                (60.0 * ((g - b) / chroma) + 360.0) % 360.0
            } else if max == g {
                60.0 * ((b - r) / chroma + 2.0)
            } else {
                60.0 * ((r - g) / chroma + 4.0)
            };
            bins[((hue / 10.0) as usize).min(35)] += 1;
        }
        if covered == 0 {
            return (None, 0.0);
        }
        let chromatic: usize = bins.iter().sum();
        if chromatic < achromatic {
            return (None, achromatic as f32 / covered as f32);
        }
        let peak = (0..36).max_by_key(|i| bins[*i]).expect("36 bins");
        let near: usize = (0..36)
            .filter(|k| {
                let d = (*k as i32 - peak as i32).abs();
                d.min(36 - d) <= 3
            })
            .map(|k| bins[k])
            .sum();
        (Some(peak as f32 * 10.0 + 5.0), near as f32 / covered as f32)
    }

    fn hue_of(kind: FileType<'_>) -> (Option<f32>, f32) {
        let icon = file_icon(kind, px(32.)).expect("a declared type has an icon");
        let bytes = icon.image.as_bytes(0).expect("one frame");
        dominant_hue(bytes)
    }

    #[test]
    fn the_folder_icon_is_blue() {
        let (hue, share) = hue_of(FileType::Folder);
        let hue = hue.expect("the folder is not a grey document sheet");
        assert!((190.0..215.0).contains(&hue), "folder hue {hue}");
        assert!(share > 0.8, "folder blue covers only {share}");
    }

    #[test]
    fn the_zip_icon_is_orange() {
        let (hue, share) = hue_of(FileType::Extension("zip"));
        let hue = hue.expect("the archive is not a grey document sheet");
        assert!((15.0..40.0).contains(&hue), "zip hue {hue}");
        assert!(share > 0.8, "zip orange covers only {share}");
    }

    #[test]
    fn a_plain_document_icon_reports_no_hue() {
        let (hue, achromatic) = hue_of(FileType::Uti("public.plain-text"));
        assert_eq!(hue, None, "plain text should carry no dominant hue");
        assert!(achromatic > 0.9, "only {achromatic} of it was achromatic");
    }

    #[test]
    fn an_extension_takes_the_declared_type_not_the_direct_lookup() {
        let ours = ut_type(true, "app").expect("app is declared");
        let identifier: Retained<NSString> = unsafe { msg_send![&*ours, identifier] };
        assert_eq!(&*identifier.to_string(), "com.apple.application-bundle");

        let class = AnyClass::get(c"UTType").expect("UTType is present");
        let direct: Option<Retained<AnyObject>> =
            unsafe { msg_send![class, typeWithFilenameExtension: ns_string!("app")] };
        let direct: Retained<NSString> =
            unsafe { msg_send![&*direct.expect("app resolves directly too"), identifier] };
        assert_eq!(
            &*direct.to_string(),
            "com.apple.application-file",
            "the two lookups agreed, so this test no longer discriminates"
        );
    }

    #[test]
    fn an_undeclared_identifier_yields_no_icon() {
        assert!(file_icon(FileType::Uti("com.apple.icon"), px(32.)).is_none());
    }

    #[test]
    fn the_buffer_is_the_requested_size_times_the_backing_scale() {
        let icon = folder_icon(px(32.)).expect("the folder always has an icon");
        let pixels = icon.image.size(0);
        assert_eq!(icon.size, size(px(32.), px(32.)));
        assert_eq!(u32::from(pixels.width), (32.0 * icon.scale) as u32);
        assert_eq!(u32::from(pixels.height), (32.0 * icon.scale) as u32);
    }

    #[test]
    fn the_second_request_for_one_icon_does_not_raster_again() {
        let before = cache_stats().file_icon_rasters;
        let first = folder_icon(px(28.)).expect("folder");
        let after_one = cache_stats().file_icon_rasters;
        let second = folder_icon(px(28.)).expect("folder");
        let after_two = cache_stats().file_icon_rasters;

        assert_eq!(after_one - before, 1, "the first call must raster");
        assert_eq!(after_two, after_one, "the second call must not");
        assert!(Arc::ptr_eq(&first.image, &second.image), "same raster");

        GENERATION.fetch_add(1, Ordering::AcqRel);
        let third = folder_icon(px(28.)).expect("folder");
        assert_eq!(cache_stats().file_icon_rasters, after_two);
        assert!(Arc::ptr_eq(&first.image, &third.image));
    }

    #[test]
    fn a_different_size_is_a_different_entry() {
        let small = folder_icon(px(16.)).expect("folder");
        let large = folder_icon(px(64.)).expect("folder");
        assert!(!Arc::ptr_eq(&small.image, &large.image));
        assert_eq!(
            u32::from(small.image.size(0).width),
            (16.0 * small.scale) as u32
        );
        assert_eq!(
            u32::from(large.image.size(0).width),
            (64.0 * large.scale) as u32
        );
    }

    #[test]
    fn twenty_thousand_extension_lookups_stay_cache_hits() {
        const EXTENSIONS: [&str; 30] = [
            "zip", "png", "jpg", "gif", "mp4", "mov", "pdf", "txt", "rtf", "html", "css", "js",
            "json", "xml", "csv", "rs", "py", "c", "h", "cpp", "swift", "m", "plist", "app", "dmg",
            "pkg", "mp3", "wav", "aiff", "tiff",
        ];
        for ext in EXTENSIONS {
            file_icon(FileType::Extension(ext), px(16.)).expect("declared extension");
        }
        let before = cache_stats().file_icon_rasters;
        let start = std::time::Instant::now();
        for i in 0..20_000usize {
            file_icon(
                FileType::Extension(EXTENSIONS[i % EXTENSIONS.len()]),
                px(16.),
            );
        }
        let elapsed = start.elapsed();
        assert_eq!(
            cache_stats().file_icon_rasters,
            before,
            "warmed-up extensions must never raster again"
        );
        eprintln!(
            "20000 file_icon(Extension, 16pt) lookups after warm-up: {:?} total, {:?} each",
            elapsed,
            elapsed / 20_000
        );
    }

    #[test]
    fn un_premultiplying_changed_the_rim() {
        let icon = folder_icon(px(32.)).expect("folder");
        let bytes = icon.image.as_bytes(0).expect("one frame");
        let partial = bytes
            .chunks_exact(4)
            .filter(|p| p[3] > 0 && p[3] < 255)
            .count();
        assert!(
            partial > 100,
            "only {partial} antialiased pixels to convert"
        );
        let over = bytes
            .chunks_exact(4)
            .filter(|p| p[3] > 0 && p[3] < 255 && p[..3].iter().any(|c| *c > p[3]))
            .count();
        assert!(
            over > 0,
            "no pixel exceeds its alpha, so nothing was divided out"
        );
    }
}
