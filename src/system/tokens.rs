//! The system's own colours, fonts, symbols and file-type icons, for the parts GPUI draws itself.
//!
//! Hosted controls get their pixels from macOS for free; the labels, separators and panel
//! backgrounds GPUI draws around them do not, and a hex value typed there is wrong the moment the
//! user changes appearance or accent.
//!
//! ```no_run
//! # use gpui::{App, div, prelude::*};
//! # use native_gpui::tokens::{self, SystemColor, system_color, system_font};
//! # fn setup(cx: &mut App) {
//! tokens::init(cx); // once, at startup
//! # }
//! # fn ui() -> impl IntoElement {
//! div()
//!     .bg(system_color(SystemColor::WindowBackground))
//!     .text_color(system_color(SystemColor::Label))
//!     .font(system_font(gpui::FontWeight::NORMAL))
//! # }
//! ```
//!
//! An `NSColor` is a recipe, not a value: reading its components without first installing a
//! drawing appearance silently returns the ambient one, so every read here goes through
//! [`NSAppearance::performAsCurrentDrawingAppearance`].
//!
//! [`init`] republishes the [`SystemTheme`] global on every system change; observe it to re-render.
//! Colour correctness does not depend on it — [`system_color`] keys its cache by effective
//! appearance on every call — but accent and display changes only propagate with it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Which of the two colour worlds a value should be resolved in.
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

    /// Collapse any appearance onto one of the two worlds, via
    /// `bestMatchFromAppearancesWithNames`: a name-string compare files
    /// `accessibilityHighContrastDarkAqua` as light.
    pub fn of(appearance: &NSAppearance) -> Appearance {
        let names = NSArray::from_slice(&[
            Appearance::Light.ns_name().as_ref(),
            Appearance::Dark.ns_name().as_ref(),
        ]);
        match appearance.bestMatchFromAppearancesWithNames(&names) {
            Some(name) if &*name == unsafe { NSAppearanceNameDarkAqua } => Appearance::Dark,
            Some(_) => Appearance::Light,
            // An appearance Apple has not shipped yet: `allowsVibrancy` is true for every dark and
            // vibrant one, false for plain aqua.
            None if appearance.allowsVibrancy() => Appearance::Dark,
            None => Appearance::Light,
        }
    }
}

thread_local! {
    /// Aqua and dark aqua are process-wide singletons, so a pointer compare hits on nearly every
    /// call and the `NSArray`-allocating match runs once.
    static APPEARANCE_MEMO: Cell<(*const NSAppearance, Appearance)> =
        const { Cell::new((ptr::null(), Appearance::Light)) };
}

/// This process's current effective appearance, honouring a [`set_app_appearance`] override.
pub fn current_appearance() -> Appearance {
    // Off the main thread there is no NSApp to ask; every caller here is a GPUI render path.
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

/// Force this process's appearance, or `None` to follow the system again, through the same KVO a
/// real system flip drives.
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
        /// A semantic macOS colour: a role, not a value. Resolve with [`system_color`] (cached,
        /// current appearance) or [`SystemColor::try_resolve`] (uncached, explicit appearance).
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
        #[non_exhaustive]
        pub enum SystemColor {
            $($variant),*
        }

        impl SystemColor {
            /// Every colour this module exposes, in a stable order.
            pub const ALL: &'static [SystemColor] = &[$(SystemColor::$variant),*];

            /// The AppKit selector this colour comes from.
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
    // text
    (Label, labelColor),
    (SecondaryLabel, secondaryLabelColor),
    (TertiaryLabel, tertiaryLabelColor),
    (QuaternaryLabel, quaternaryLabelColor),
    (QuinaryLabel, quinaryLabelColor),
    (Text, textColor),
    (PlaceholderText, placeholderTextColor),
    (HeaderText, headerTextColor),
    // text fields and selection
    (TextBackground, textBackgroundColor),
    (SelectedText, selectedTextColor),
    (SelectedTextBackground, selectedTextBackgroundColor),
    (
        UnemphasizedSelectedTextBackground,
        unemphasizedSelectedTextBackgroundColor
    ),
    (TextInsertionPoint, textInsertionPointColor),
    // surfaces
    (WindowBackground, windowBackgroundColor),
    (UnderPageBackground, underPageBackgroundColor),
    (ControlBackground, controlBackgroundColor),
    // controls
    (Control, controlColor),
    (ControlText, controlTextColor),
    (DisabledControlText, disabledControlTextColor),
    (SelectedControl, selectedControlColor),
    (SelectedControlText, selectedControlTextColor),
    (
        AlternateSelectedControlText,
        alternateSelectedControlTextColor
    ),
    // lists and tables
    (SelectedContentBackground, selectedContentBackgroundColor),
    (
        UnemphasizedSelectedContentBackground,
        unemphasizedSelectedContentBackgroundColor
    ),
    // lines and emphasis
    (Separator, separatorColor),
    (Grid, gridColor),
    (Link, linkColor),
    (ControlAccent, controlAccentColor),
    (KeyboardFocusIndicator, keyboardFocusIndicatorColor),
    (FindHighlight, findHighlightColor),
    (Shadow, shadowColor),
    // fills
    (SystemFill, systemFillColor),
    (SecondarySystemFill, secondarySystemFillColor),
    (TertiarySystemFill, tertiarySystemFillColor),
    (QuaternarySystemFill, quaternarySystemFillColor),
    (QuinarySystemFill, quinarySystemFillColor),
    // palette
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
    /// The live `NSColor` for handing a semantic colour back to AppKit, where `Hsla` would lose the
    /// recipe.
    pub fn ns_color(self) -> Retained<NSColor> {
        (self.getter())()
    }

    /// Resolve for an explicit appearance, uncached.
    ///
    /// `None` when sRGB conversion refuses, as it still does for pattern colours
    /// (`NSPatternColor`): `redComponent` off one throws or yields a plausible-looking black, so
    /// the `Option` has to stay. [`refused`] says which ones.
    pub fn try_resolve(self, appearance: Appearance) -> Option<Hsla> {
        let getter = self.getter();
        with_appearance(appearance, || from_nscolor(&getter()))
    }
}

/// Run `f` with `appearance` installed as the current drawing appearance. Panics if the block never
/// ran, which would silently degrade a two-appearance comparison into two identical reads.
fn with_appearance<T>(appearance: Appearance, f: impl Fn() -> T) -> T {
    let slot: RefCell<Option<T>> = RefCell::new(None);
    let block = StackBlock::new(|| *slot.borrow_mut() = Some(f()));
    appearance
        .ns_appearance()
        .performAsCurrentDrawingAppearance(&block);
    slot.into_inner()
        .expect("performAsCurrentDrawingAppearance never invoked the block")
}

/// `labelColor`'s value, so a colour that refuses sRGB conversion degrades to readable text.
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

/// Which colours refuse sRGB conversion under `appearance` — none, as of macOS 26. Assert it is
/// empty at startup to turn `refusal_fallback`'s substitution into a tripwire.
pub fn refused(appearance: Appearance) -> Vec<SystemColor> {
    SystemColor::ALL
        .iter()
        .copied()
        .filter(|c| c.try_resolve(appearance).is_none())
        .collect()
}

/// Bumped by every observer in [`init`], and by [`set_app_appearance`] through KVO.
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The live token generation. [`CacheStats::generation`] is the one the cache is holding.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[derive(Default)]
struct Cache {
    generation: u64,
    palettes: [Option<Vec<Option<Hsla>>>; 2],
    /// Max backing scale across all displays, so a symbol survives a move to a Retina one.
    scale: Option<f32>,
    symbols: HashMap<SymbolKey, Option<Symbol>>,
    /// Outlives the generation, unlike `symbols`: a file icon is fixed artwork keyed by its own
    /// scale, so nothing a token observer watches can change one.
    file_icons: HashMap<FileIconKey, Option<Symbol>>,
    palette_resolves: u64,
    symbol_rasters: u64,
    file_icon_rasters: u64,
}

/// Counters, not gauges. The palette cache is keyed by appearance, so only re-entering an
/// appearance distinguishes a live cache from a stale one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CacheStats {
    /// One `performAsCurrentDrawingAppearance` pass per appearance per generation.
    pub palette_resolves: u64,
    /// Including the ones that came back `None`.
    pub symbol_rasters: u64,
    /// Never reset, because [`file_icon`] survives a generation bump.
    pub file_icon_rasters: u64,
    /// Lags [`generation`] until the next token read.
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
            // `file_icons` deliberately survives: appearance, accent and display changes cannot
            // alter fixed artwork, and its key already carries the backing scale.
            cache.symbols.clear();
        }
        f(&mut cache)
    })
}

/// A semantic colour for this process's current appearance. The first call for an appearance
/// resolves all of [`SystemColor::ALL`] in one pass; every later call is an array index.
///
/// A [`Layer::Below`](crate::Layer::Below) `NSVisualEffectView` only shows through where GPUI
/// painted nothing, so an opaque `.bg(system_color(..))` over a material silently replaces the blur
/// with an approximation of it — paint nothing, or below alpha 1.
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

/// The user's accent colour. Shorthand for [`SystemColor::ControlAccent`].
pub fn accent() -> Hsla {
    system_color(SystemColor::ControlAccent)
}

/// **The** window background: the one colour every opaque surface of a window is painted with —
/// the toolbar band, the content pane behind it, and whatever an app draws in the body.
///
/// One value, because two are visible as a step. A window's toolbar band and its body are the
/// same surface interrupted by a strip of chrome, and anything that resolves them differently
/// draws a line across the window where no line exists. The one deliberate exception is a
/// sidebar, which is vibrancy over this and not a colour at all; see
/// [`SidebarOptions::backdrop`](crate::sidebar::SidebarOptions::backdrop).
///
/// It has to be an opaque colour and not a material. A material's value tracks whatever it
/// samples: `UnderWindowBackground` over a flat grey 29 reads 38 and over grey 60 reads 44, while
/// `windowBackgroundColor` reads 30 over both. The window-spanning backdrop samples *behind the
/// window*, so what it tracks is the desktop — a band on that and a body on a colour cannot be
/// reconciled by any constant, because only one of them moves. Two cheap confirmations that a
/// surface is the first kind: it is state-dependent, shifting when the window stops being key
/// (43 to 55 in dark), and a colour is not.
///
/// Do not try to re-measure any of that from a window-scoped screen capture. It composites over
/// nothing behind the window, so a behind-window material reports its own flat tint however
/// colourful the desktop is, and the null reads as "not sampling".
pub const WINDOW_BACKGROUND: SystemColor = SystemColor::WindowBackground;

/// [`WINDOW_BACKGROUND`] for this process's current appearance. `WINDOW_BACKGROUND.ns_color()` is
/// the same token for a caller handing it back to AppKit.
pub fn window_background() -> Hsla {
    system_color(WINDOW_BACKGROUND)
}

fn display_scale() -> f32 {
    with_cache(|cache| {
        *cache.scale.get_or_insert_with(|| {
            let Some(mtm) = MainThreadMarker::new() else {
                return 2.0;
            };
            NSScreen::screens(mtm)
                .iter()
                .map(|s| s.backingScaleFactor() as f32)
                .fold(1.0f32, f32::max)
        })
    })
}

/// The system's visual state, republished as a GPUI global whenever it changes; observe it with
/// `cx.observe_global::<SystemTheme>(..)`. `generation` increments on every observed change,
/// including ones that leave `appearance` and `accent` untouched.
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

/// All of them, because they do not overlap cleanly: an accent-only change posts
/// `AppleColorPreferencesChanged` and leaves `effectiveAppearance` alone, so the KVO never sees it.
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
    /// KVO sink for `NSApp.effectiveAppearance` — the only observer that catches an app-local
    /// [`set_app_appearance`].
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

/// Invalidate every cache and republish [`SystemTheme`].
///
/// `set_global` is deferred through `spawn` because a GPUI-sourced appearance change arrives while
/// GPUI still holds its `App` borrow, and a synchronous update there drops the event.
fn bump(cx: &AsyncApp) {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    APPEARANCE_MEMO.with(|memo| memo.set((ptr::null(), Appearance::Light)));
    let cx2 = cx.clone();
    cx.spawn(async move |_| {
        let _ = cx2.update(|app| app.set_global(snapshot()));
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

    let already = OBSERVERS.with(|slot| slot.borrow().is_some());
    if already {
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

/// The family string that reaches the real system face through GPUI's text system.
///
/// GPUI resolves the right face but does not apply SF's optical-size (`opsz`) axis, which CoreText
/// applies whenever you ask `NSFont` for a size: about 2.6% narrow at 13pt, exact around 16pt,
/// wider above. Measure at the size you ship with [`advance_delta`].
pub const fn system_font_family() -> &'static str {
    ".AppleSystemUIFont"
}

/// The name `SF Pro Text`, whether or not this machine has it. Prefer [`text_font_family`].
pub const fn text_font_family_name() -> &'static str {
    "SF Pro Text"
}

/// `SF Pro Text` **if it is actually installed**, else `None` — a developer download rather than
/// part of a stock macOS install.
///
/// A manual stand-in for the optical-size axis GPUI does not apply: closer to AppKit up to about
/// 14pt, further away above. Falling back to [`system_font_family`] keeps forward compatibility.
pub fn text_font_family(cx: &App) -> Option<&'static str> {
    let text_system = cx.text_system();
    let resolve = |family: &str| {
        let mut font = system_font(FontWeight::NORMAL);
        font.family = family.to_string().into();
        text_system.resolve_font(&font)
    };
    // GPUI resolves an unknown family to a silent fallback rather than failing, so asking whether
    // the name resolves proves nothing. Two nonexistent families pin the fallback id.
    let fallback = resolve("NG-No-Such-Family-9x7");
    if fallback != resolve("NG-Also-Not-Installed-4b2") {
        return None;
    }
    (resolve(text_font_family_name()) != fallback).then(text_font_family_name)
}

/// `NSFont.systemFontSize`, read from the system rather than assumed.
pub fn system_font_size() -> Pixels {
    px(NSFont::systemFontSize() as f32)
}

/// `NSFont.smallSystemFontSize`.
pub fn small_system_font_size() -> Pixels {
    px(NSFont::smallSystemFontSize() as f32)
}

/// The font size that belongs to a [`ControlSize`], for a GPUI label beside a control sized the
/// same way.
///
/// AppKit answers with only three distinct values, 13pt for `Regular`, `Large` and `ExtraLarge`
/// alike — the system's answer, not a stub. Hardcoding 15 or 17 puts the label out of step.
pub fn system_font_size_for(size: ControlSize) -> Pixels {
    px(NSFont::systemFontSizeForControlSize(size.into()) as f32)
}

/// `NSFont.labelFontSize`, for chart and axis labels — not a control's own label, which is
/// [`system_font_size_for`].
pub fn label_font_size() -> Pixels {
    px(NSFont::labelFontSize() as f32)
}

/// A GPUI [`Font`] in the system face at the given weight.
pub fn system_font(weight: FontWeight) -> Font {
    Font {
        family: system_font_family().into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

/// The OpenType tag that turns SF's proportional figures into tabular ones. `monospacedDigit`
/// would name the AppKit call rather than the thing being asked for.
const TABULAR_FIGURES: &str = "tnum";

/// [`system_font`] with tabular figures, for a column of numbers that must not shift as its digits
/// change — a date column, a size column, a clock.
///
/// The same face and weight, not a monospaced one: only the digits change width. In SF at 14pt the
/// proportional `1` advances 6.34pt against `4`'s 8.86pt, so `11:11 AM` renders 6.97pt narrower
/// than `10:00 AM` and every row of a date column starts somewhere different. With this the two
/// are identical, and each digit advances 8.67pt — the same number AppKit's own
/// `monospacedDigitSystemFont(ofSize: 14, weight: .regular)` produces.
///
/// Verified by `examples/tabular_figures.rs`, which the close-out gate runs: GPUI applies the
/// feature through a `CTFontDescriptor`, so a toolchain that stopped honouring it would leave this
/// silently returning a proportional font.
pub fn system_font_tabular(weight: FontWeight) -> Font {
    Font {
        features: FontFeatures(Arc::new(vec![(TABULAR_FIGURES.to_string(), 1)])),
        ..system_font(weight)
    }
}

/// GPUI's nine weights mapped onto AppKit's, by nearest ordinal: SF Pro's `usWeightClass` values
/// are 100..900 in exactly these steps and [`FontWeight`] is the same CSS scale.
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

/// AppKit's own metrics for the system font, in points. Use these to align a GPUI label's baseline
/// with a hosted control's; GPUI's own come from a face without the optical-size axis applied.
#[derive(Clone, PartialEq, Debug)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub cap_height: f32,
    pub x_height: f32,
    /// The face AppKit actually resolved, e.g. `.AppleSystemUIFontDemi` for semibold.
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

/// The `NSFont` behind a GPUI family string, `None` for a family AppKit does not have.
/// [`system_font_family`] routes through `systemFont(ofSize:weight:)` so the `opsz` axis applies.
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

/// Typographic width of `text` in `font`, kerning and ligatures off.
///
/// `NSAttributedString.size()` does not reliably shape an OpenType feature, so these widths are
/// only trustworthy for a font whose metrics are in the face itself. Asked about
/// `monospacedDigitSystemFont(14)` it answers 63.44 for ten `1`s and 88.59 for ten `4`s — the
/// proportional advances — while `CTLine` shapes both at 86.68; and whether it takes the shaping
/// path at all turns on the string, since it *does* return the tabular width for
/// `11:11 AM`. On every font without a feature the two agree exactly: 0.000000 across 22 strings
/// at four sizes, trailing and leading spaces included.
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

/// AppKit's own width for a whole string, in points, kerning and ligatures off. Do not measure a
/// font carrying an OpenType feature with this — see [`system_font_tabular`], and use GPUI's own
/// `shape_line` for those.
pub fn appkit_string_width(
    family: &str,
    size: Pixels,
    weight: FontWeight,
    text: &str,
) -> Option<f32> {
    Some(typeset_width(&*ns_font(family, size, weight)?, text) as f32)
}

/// AppKit's advance width for one character, in points.
///
/// Differential — `width(ch ch) - width(ch)` — to cancel the string's side bearings. The direct
/// route, `NSFont.glyphWithName`, takes a **glyph name** rather than a character: it answers for
/// `'x'` and returns glyph 0 for `'0'` and `' '`, silently dropping every digit and space.
pub fn appkit_advance(family: &str, size: Pixels, weight: FontWeight, ch: char) -> Option<f32> {
    let font = ns_font(family, size, weight)?;
    Some(
        (typeset_width(&font, &format!("{ch}{ch}")) - typeset_width(&font, &ch.to_string())) as f32,
    )
}

/// GPUI's advance for `ch` in `family`, in pixels. The other arm of [`advance_delta`], exposed
/// separately because a surprising delta is only diagnosable by reading each side on its own.
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

/// How far GPUI's advance for `ch` is from AppKit's, in points. Positive means GPUI is wider.
///
/// Run a named family as a control: if that disagrees too, the two instruments are incomparable.
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
    /// `labelColor`, hierarchically — what an `NSImageView` does with a template symbol.
    Label,
    /// One colour, with AppKit deriving reduced-intensity secondary and tertiary layers from it.
    Hierarchical(SystemColor),
    /// One colour per layer, in order. Layers past the end of the list keep the last colour.
    Palette(Vec<SystemColor>),
    /// The symbol's own multicolour artwork where it has one, monochrome where it does not.
    Multicolor,
    /// No colour configuration at all, for symbols that ship their own fixed artwork. A template
    /// symbol comes out **black**, which is invisible in dark mode.
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

/// A rasterised system image, ready to hand to GPUI: an SF Symbol from [`sf_symbol`] or a file
/// type's own artwork from [`file_icon`]. One type for both, so a list row that shows either can
/// hold one field.
#[derive(Clone)]
pub struct Symbol {
    pub image: Arc<RenderImage>,
    /// Logical size in points; the buffer is `scale` times this, and laying the image out at
    /// anything else resamples it.
    pub size: Size<Pixels>,
    pub scale: f32,
}

impl Symbol {
    /// The symbol as a correctly-sized GPUI element.
    pub fn img(&self) -> Img {
        img(self.image.clone())
            .w(self.size.width)
            .h(self.size.height)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SymbolKey {
    name: String,
    /// `Pixels` is `f32`, which is not `Hash`. Bits are exact for a cache key.
    size: u32,
    weight: u32,
    color: SymbolColor,
    appearance: Appearance,
}

/// Render an SF Symbol for GPUI, in `labelColor` at the current appearance. `None` if the system
/// has no symbol by that name: a placeholder glyph would hide a name macOS 26 has and an older
/// release does not.
pub fn sf_symbol(name: &str, size: Pixels, weight: FontWeight) -> Option<Symbol> {
    sf_symbol_colored(name, size, weight, &SymbolColor::Label)
}

/// [`sf_symbol`] with an explicit colour configuration. Cached by every input including appearance
/// and dropped on any system change: a semantic tint resolves at raster time, so the same symbol
/// is a different image under the other appearance.
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

    // A semantic tint resolves at raster time, so outside the drawing appearance a `labelColor`
    // symbol picks up the ambient one and comes out black on a dark panel.
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

/// Which system icon to fetch. Every variant is a *type* lookup — there is deliberately no
/// path-taking form, so no icon here depends on a file existing or on reading one.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FileType<'a> {
    /// The blue folder, via `public.folder`.
    Folder,
    /// A filename extension without the dot, e.g. `"zip"`.
    Extension(&'a str),
    /// A uniform type identifier, e.g. `"public.plain-text"`.
    Uti(&'a str),
}

impl<'a> FileType<'a> {
    /// `(is_extension, text)` — the two things the lookup actually varies on.
    fn tag(self) -> (bool, &'a str) {
        match self {
            FileType::Folder => (false, "public.folder"),
            FileType::Uti(uti) => (false, uti),
            FileType::Extension(extension) => (true, extension),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FileIconKey {
    /// Distinguishes [`FileType::Extension`] from [`FileType::Uti`], which can spell the same
    /// string and mean different types.
    extension: bool,
    text: String,
    /// `Pixels` is `f32`, which is not `Hash`. Bits are exact for a cache key.
    size: u32,
    /// Part of the key rather than something a generation bump clears: a display change alters
    /// only the buffer resolution, and keying it means the old raster stays valid if the user
    /// moves the window back.
    scale: u32,
}

/// The system's own artwork for a file type, rasterised for GPUI: the blue folder, the orange
/// `.zip`, the document sheet with a type badge. `None` if `UTType` cannot resolve the type at all
/// — a placeholder would be indistinguishable from a real generic-document icon.
///
/// Cached by type, requested size and backing scale, and — unlike [`sf_symbol`] — **not** by
/// appearance: the four artwork sets measured here are byte-identical under aqua and dark aqua,
/// where the same comparison on a `labelColor` symbol moves every antialiased pixel. Nothing a
/// token observer watches can change one, so a generation bump leaves the cache standing. The one
/// thing that would invalidate an entry is the Launch Services type database changing under the
/// process — an app installed while it runs, claiming an extension — and this crate does not
/// observe that.
///
/// One lookup plus raster costs around 42µs, nearly all of it the raster, so a list re-requesting
/// per row per frame would spend more on icons than on everything else it draws.
///
/// ```no_run
/// # use native_gpui::tokens::{FileType, file_icon};
/// # use gpui::px;
/// # fn row() -> Option<gpui::Img> {
/// Some(file_icon(FileType::Extension("zip"), px(32.))?.img())
/// # }
/// ```
pub fn file_icon(kind: FileType<'_>, size: Pixels) -> Option<Symbol> {
    let (extension, text) = kind.tag();
    let scale = display_scale();
    let key = FileIconKey {
        extension,
        text: text.to_string(),
        size: f32::from(size).to_bits(),
        scale: scale.to_bits(),
    };
    with_cache(|cache| match cache.file_icons.get(&key) {
        Some(hit) => hit.clone(),
        None => {
            cache.file_icon_rasters += 1;
            let icon = rasterize_file_icon(extension, text, size, scale);
            cache.file_icons.insert(key, icon.clone());
            icon
        }
    })
}

/// The folder icon at `size`. Shorthand for [`FileType::Folder`], and the only folder form there
/// is: a per-folder custom icon would need a path.
pub fn folder_icon(size: Pixels) -> Option<Symbol> {
    file_icon(FileType::Folder, size)
}

/// `UTType` for an extension or a uniform type identifier, `None` if the class is missing or the
/// identifier is not declared.
///
/// `typeWithFilenameExtension:` is the obvious call for an extension and it is the wrong one. For
/// `icon` and `iconset` it answers a *dynamic* `dyn.ah62…` type, whose icon is the blank document
/// sheet, while `typesWithTag:tagClass:conformingToType:` finds the declared
/// `com.apple.iconcomposer.icon` in the same process. It also disagrees on `app`, answering the
/// legacy single-file `com.apple.application-file` where the tag search leads with
/// `com.apple.application-bundle`. So the first declared match wins, and the direct call is only
/// the fallback for an extension nothing claims.
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

fn rasterize_file_icon(extension: bool, text: &str, size: Pixels, scale: f32) -> Option<Symbol> {
    let kind = ut_type(extension, text)?;
    let workspace = NSWorkspace::sharedWorkspace();
    let image: Option<Retained<NSImage>> =
        unsafe { msg_send![&*workspace, iconForContentType: &*kind] };
    let image = image?;

    // `iconForContentType:` reports 32x32 points however much it holds — 32 representations up to
    // 2048px — so its own `size()` is no guide and the requested size is what to draw into.
    // `drawInRect:` then picks a representation by **destination pixels**, not by the point rect:
    // 32pt into a 2x buffer and 64pt into a 1x buffer come out byte-identical. The rungs are
    // 16, 18, 24, 32, 36, 48, 64, 128, 256, 512, 1024 and 2048 pixels, so 32pt at 2x lands
    // exactly on 64 and a size off the ladder resamples.
    let points = NSSize::new(f32::from(size) as f64, f32::from(size) as f64);
    let w = ((points.width * scale as f64).round() as usize).max(1);
    let h = ((points.height * scale as f64).round() as usize).max(1);

    // No drawing appearance, unlike a symbol: this is fixed artwork, and installing one was
    // measured to change nothing.
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

/// Draw an `NSImage` into a fresh premultiplied RGBA bitmap.
///
/// The rep's *size* is in points while its buffer is in pixels; that mismatch is what gives the 2x
/// transform, and without it the glyph rasterises into the top-left quarter.
fn draw(image: &NSImage, points: NSSize, w: usize, h: usize) -> Option<Vec<u8>> {
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
    rep.setSize(points);

    // A bitmap rep with no current graphics context accepts `drawInRect` and silently produces an
    // all-zero buffer.
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

/// AppKit's premultiplied RGBA to the straight-alpha BGRA gpui's `RenderImage` expects: swap R/B
/// **and** divide out alpha.
///
/// Swapping and stopping there is the trap: interiors are byte-identical (dividing by `a = 255` is
/// the identity), so only antialiased edges differ, reading as a dark fringe around every glyph —
/// and a black template symbol cannot show it at all.
fn to_bgra_straight(rgba: &[u8]) -> Vec<u8> {
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

    /// Dominant hue in degrees over the opaque pixels of a **BGRA** buffer, and the share of them
    /// within 30 degrees of it. `None` when the artwork carries no colour, which is the answer for
    /// every plain document sheet and is what makes a blue or orange verdict falsifiable.
    ///
    /// Reading the same buffer as RGBA would swap the verdicts — a folder would report orange —
    /// so a correct hue here also witnesses the channel order [`to_bgra_straight`] writes.
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

    /// The control for the two above: the same metric on artwork that carries no hue must answer
    /// `None`, or a blue and an orange verdict prove nothing about the raster.
    #[test]
    fn a_plain_document_icon_reports_no_hue() {
        let (hue, achromatic) = hue_of(FileType::Uti("public.plain-text"));
        assert_eq!(hue, None, "plain text should carry no dominant hue");
        assert!(achromatic > 0.9, "only {achromatic} of it was achromatic");
    }

    /// Guards the tag search in [`ut_type`]. Simplifying it to the one obvious call still compiles,
    /// still returns an icon, and silently answers with the legacy single-file application type.
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

        // A generation bump drops symbols; file icons are fixed artwork and stay.
        GENERATION.fetch_add(1, Ordering::AcqRel);
        let third = folder_icon(px(28.)).expect("folder");
        assert_eq!(cache_stats().file_icon_rasters, after_two);
        assert!(Arc::ptr_eq(&first.image, &third.image));
    }

    /// A different size is a different raster, so the size really is part of the key.
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

    /// The straight-alpha conversion has to have done something, or the anchored fact it protects
    /// is untested on this path: a file icon's antialiased rim is where it shows.
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
        // Straight alpha lets a channel exceed its own alpha; premultiplied never can.
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
