//! The system's own colours, fonts and symbols, for the parts GPUI draws itself.
//!
//! Hosted controls get their pixels from macOS for free. Everything *around* them — panel
//! backgrounds, labels, separators, icons — is drawn by GPUI, and a hand-picked hex value there is
//! frozen the day it is typed and wrong the moment the user changes appearance or accent. This
//! module reads those values out of AppKit instead.
//!
//! ```no_run
//! # use gpui::{App, div, prelude::*};
//! # use native_gpui::tokens::{self, SystemColor, SystemTheme, system_color, system_font};
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
//! An `NSColor` is a recipe, not a value, and reading its components without first installing a
//! drawing appearance silently returns whatever the ambient one happens to be — correct on the
//! machine you tested on and wrong on the other. Every read here goes through
//! [`NSAppearance::performAsCurrentDrawingAppearance`].
//!
//! [`init`] registers the notifications and KVO behind appearance and accent changes and
//! republishes the [`SystemTheme`] global; observe that to re-render. Appearance *correctness* does
//! not depend on it — [`system_color`] keys its cache by the process's effective appearance on
//! every call — but accent changes and display reconfigurations only propagate with it.

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
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceName, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSAttributedStringNSStringDrawing, NSBitmapFormat, NSBitmapImageRep, NSColor, NSColorSpace,
    NSDeviceRGBColorSpace, NSFont, NSFontAttributeName, NSFontWeight, NSFontWeightBlack,
    NSFontWeightBold, NSFontWeightHeavy, NSFontWeightLight, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSFontWeightThin, NSFontWeightUltraLight,
    NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSImageSymbolScale,
    NSKernAttributeName, NSLigatureAttributeName, NSScreen,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSDictionary, NSDistributedNotificationCenter,
    NSKeyValueObservingOptions, NSNotification, NSNotificationCenter, NSNumber,
    NSObjectNSKeyValueObserverRegistration, NSPoint, NSRect, NSSize, NSString, ns_string,
};

use crate::bridge::host::ControlSize;

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

    /// Collapse any appearance onto one of the two worlds.
    ///
    /// Via `bestMatchFromAppearancesWithNames`, the way AppKit does it: comparing appearance names
    /// by string would file `accessibilityHighContrastDarkAqua` as light.
    pub fn of(appearance: &NSAppearance) -> Appearance {
        let names = NSArray::from_slice(&[
            Appearance::Light.ns_name().as_ref(),
            Appearance::Dark.ns_name().as_ref(),
        ]);
        match appearance.bestMatchFromAppearancesWithNames(&names) {
            Some(name) if &*name == unsafe { NSAppearanceNameDarkAqua } => Appearance::Dark,
            Some(_) => Appearance::Light,
            // Matches neither, so it is an appearance Apple has not shipped yet. `allowsVibrancy`
            // is true for every dark and vibrant one and false for plain aqua.
            None if appearance.allowsVibrancy() => Appearance::Dark,
            None => Appearance::Light,
        }
    }
}

thread_local! {
    /// `NSAppearance` instances for aqua and dark aqua are process-wide singletons, so a pointer
    /// compare hits on essentially every call and the `NSArray`-allocating match runs once.
    static APPEARANCE_MEMO: Cell<(*const NSAppearance, Appearance)> =
        const { Cell::new((ptr::null(), Appearance::Light)) };
}

/// This process's current effective appearance, honouring a [`set_app_appearance`] override.
pub fn current_appearance() -> Appearance {
    // Off the main thread there is no NSApp to ask. Every caller here is a GPUI render path.
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

/// Force this process's appearance, or pass `None` to follow the system again.
///
/// Drives the same KVO a real system flip does, so [`SystemTheme`] and every cache here update
/// through the user's path rather than a parallel one.
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
    // the crayon box
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
    /// The live `NSColor`, for handing a semantic colour back to AppKit — a symbol tint, a
    /// control's `contentTintColor` — where `Hsla` would lose the recipe.
    pub fn ns_color(self) -> Retained<NSColor> {
        (self.getter())()
    }

    /// Resolve for an explicit appearance, uncached.
    ///
    /// `None` when `colorUsingColorSpace(sRGB)` refuses, which AppKit still does for pattern
    /// colours (`NSPatternColor`); reading `redComponent` off one throws or yields a
    /// plausible-looking black, so the `Option` has to stay. [`refused`] says which ones.
    pub fn try_resolve(self, appearance: Appearance) -> Option<Hsla> {
        let getter = self.getter();
        with_appearance(appearance, || to_srgb(&getter()))
    }

    /// Resolve with **no** explicit drawing appearance — whatever the ambient one happens to be.
    ///
    /// Exposed only so a test can show [`try_resolve`](Self::try_resolve) is doing something: this
    /// read matches one appearance and silently contradicts the other. Never use it in a UI.
    pub fn resolve_ambient(self) -> Option<Hsla> {
        to_srgb(&self.ns_color())
    }
}

/// Run `f` with `appearance` installed as the current drawing appearance.
///
/// Panics if AppKit never invoked the block; without that, comparing two appearances silently
/// degrades into two reads of the same one.
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
///
/// For colours outside [`SystemColor`]. It reads whatever drawing appearance is ambient at the
/// call, so for a semantic colour reach for [`SystemColor::try_resolve`] instead.
pub fn srgb(color: &NSColor) -> Option<Hsla> {
    to_srgb(color)
}

fn to_srgb(color: &NSColor) -> Option<Hsla> {
    let space = NSColorSpace::sRGBColorSpace();
    let converted = color.colorUsingColorSpace(&space)?;
    let (mut r, mut g, mut b, mut a) = (0f64, 0f64, 0f64, 0f64);
    unsafe { converted.getRed_green_blue_alpha(&mut r, &mut g, &mut b, &mut a) };
    Some(
        Rgba {
            r: r as f32,
            g: g as f32,
            b: b as f32,
            a: a as f32,
        }
        .into(),
    )
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
/// empty at startup to turn [`refusal_fallback`]'s substitution into a tripwire.
pub fn refused(appearance: Appearance) -> Vec<SystemColor> {
    SystemColor::ALL
        .iter()
        .copied()
        .filter(|c| c.try_resolve(appearance).is_none())
        .collect()
}

/// Bumped by every observer in [`init`], and by [`set_app_appearance`] through KVO.
///
/// Atomic rather than a `RefCell` because an AppKit notification can arrive while GPUI holds its
/// `App` borrow — see [`bump`].
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The live token generation. [`CacheStats::generation`] is the one the cache is holding.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[derive(Default)]
struct Cache {
    generation: u64,
    palettes: [Option<Vec<Option<Hsla>>>; 2],
    /// Max backing scale across all displays, so a symbol is never under-resolved after the window
    /// moves to a Retina one.
    scale: Option<f32>,
    symbols: HashMap<SymbolKey, Option<Symbol>>,
    palette_resolves: u64,
    symbol_rasters: u64,
}

/// Work the cache actually did — counters, not gauges. The palette cache is keyed by appearance, so
/// a flip returns a correct value even with invalidation broken; only re-entering an appearance
/// distinguishes a live cache from a stale one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CacheStats {
    /// Full `performAsCurrentDrawingAppearance` passes — one per appearance per generation.
    pub palette_resolves: u64,
    /// SF Symbol rasterisations, including the ones that came back `None`.
    pub symbol_rasters: u64,
    /// The generation the cache holds, which lags [`generation`] until the next token read.
    pub generation: u64,
}

pub fn cache_stats() -> CacheStats {
    CACHE.with(|cache| {
        let cache = cache.borrow();
        CacheStats {
            palette_resolves: cache.palette_resolves,
            symbol_rasters: cache.symbol_rasters,
            generation: cache.generation,
        }
    })
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// Borrow the cache, clearing it first if the system has changed under us.
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

/// A semantic colour, resolved for this process's current appearance.
///
/// The first call for an appearance resolves all of [`SystemColor::ALL`] in one
/// `performAsCurrentDrawingAppearance` pass; every later call is an array index.
///
/// These are flat values. A [`Layer::Below`](crate::Layer::Below) `NSVisualEffectView` only shows
/// through where GPUI painted nothing, so an opaque `.bg(system_color(..))` over a material
/// silently replaces the blur with an approximation of it — paint nothing, or below alpha 1.
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
                        .map(|c| to_srgb(&c.ns_color()))
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

/// The system's visual state, republished as a GPUI global whenever it changes.
///
/// `cx.observe_global::<SystemTheme>(..)` to re-render. `generation` increments on every observed
/// change, including ones that leave `appearance` and `accent` untouched.
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
    /// [`set_app_appearance`], and the only one that fires before the first frame is already wrong.
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
/// `set_global` is deferred through `spawn` because an appearance change reached from a GPUI
/// listener — which is what a Light/Dark toggle is — arrives while GPUI still holds its `App`
/// borrow, and updating synchronously there drops the event.
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
/// applies whenever you ask `NSFont` for a size. So a GPUI label here does not track an adjacent
/// AppKit one: about 2.6% narrow at 13pt, exact around 16pt, wider above. Measure at the size you
/// ship with [`advance_delta`].
pub const fn system_font_family() -> &'static str {
    ".AppleSystemUIFont"
}

/// The name `SF Pro Text`, whether or not this machine has it. Prefer [`text_font_family`].
pub const fn text_font_family_name() -> &'static str {
    "SF Pro Text"
}

/// `SF Pro Text` **if it is actually installed**, else `None` — it is a developer download rather
/// than part of a stock macOS install.
///
/// Naming the Text cut is a manual stand-in for the optical-size axis GPUI does not apply, and it
/// lands closer to AppKit up to about 14pt and further away above. It also forfeits the forward
/// compatibility of [`system_font_family`], which is the fallback to spell out:
/// `text_font_family(cx).unwrap_or_else(system_font_family)`.
pub fn text_font_family(cx: &App) -> Option<&'static str> {
    let text_system = cx.text_system();
    let resolve = |family: &str| {
        let mut font = system_font(FontWeight::NORMAL);
        font.family = family.to_string().into();
        text_system.resolve_font(&font)
    };
    // GPUI resolves an unknown family to a silent fallback rather than failing, so asking whether
    // the name resolves proves nothing. Two independently nonexistent families pin the fallback id.
    let fallback = resolve("NG-No-Such-Family-9x7");
    let fallback_again = resolve("NG-Also-Not-Installed-4b2");
    if fallback != fallback_again {
        // The discriminator itself is broken, so refuse rather than guess.
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

/// The font size that belongs to a given [`ControlSize`], for a GPUI label beside a control you
/// sized the same way.
///
/// AppKit answers with only three distinct values, 13pt for `Regular`, `Large` and `ExtraLarge`
/// alike. That is the system's answer and not a stub: hardcoding 15 or 17 for the large sizes would
/// put a GPUI label out of step with the control beside it.
pub fn system_font_size_for(size: ControlSize) -> Pixels {
    px(NSFont::systemFontSizeForControlSize(size.into()) as f32)
}

/// `NSFont.labelFontSize`, the size AppKit sets chart and axis labels at. Not the size of a
/// control's own label; that is [`system_font_size_for`].
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

/// GPUI's nine weights mapped onto AppKit's.
///
/// Ordinal, because SF Pro's `usWeightClass` values are 100..900 in exactly these steps and
/// [`FontWeight`] is the same CSS scale.
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

/// AppKit's own metrics for the system font, in points.
///
/// Use these to align a GPUI-drawn label's baseline with a hosted control's: GPUI's own ascent and
/// descent come from a face without the optical-size axis applied.
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
///
/// [`system_font_family`] routes to `systemFont(ofSize:weight:)` so the optical-size axis is
/// applied; anything else goes through `NSFont(name:size:)`.
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

/// AppKit's own width for a whole string, in points, kerning and ligatures off.
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
/// Measured differentially as `width(ch ch) - width(ch)`, which cancels the string's side bearings.
/// The direct route, `NSFont.glyphWithName`, takes a **glyph name** rather than a character: it
/// answers correctly for `'x'` and returns glyph 0 for `'0'` (named `zero`) and `' '` (`space`), so
/// summing it over a real string silently drops every digit and space.
pub fn appkit_advance(family: &str, size: Pixels, weight: FontWeight, ch: char) -> Option<f32> {
    let font = ns_font(family, size, weight)?;
    Some(
        (typeset_width(&font, &format!("{ch}{ch}")) - typeset_width(&font, &ch.to_string())) as f32,
    )
}

/// GPUI's advance for `ch` in `family`, in pixels.
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
/// Run it on a named family as a control: if one of those disagrees too, the two instruments are
/// simply incomparable and nothing can be concluded about the system font.
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

/// A rasterised SF Symbol, ready to hand to GPUI.
#[derive(Clone)]
pub struct Symbol {
    pub image: Arc<RenderImage>,
    /// Logical size in points. The buffer is `scale` times this in each axis; laying the image out
    /// at anything else resamples it.
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

/// Render an SF Symbol for GPUI, in `labelColor` at the current appearance.
///
/// `None` if the system has no symbol by that name, because a name that exists on macOS 26 may not
/// exist on an older one and a placeholder glyph would hide that.
pub fn sf_symbol(name: &str, size: Pixels, weight: FontWeight) -> Option<Symbol> {
    sf_symbol_colored(name, size, weight, &SymbolColor::Label)
}

/// [`sf_symbol`] with an explicit colour configuration.
///
/// Cached by every input including appearance, and dropped on any system change: a semantic tint
/// resolves at raster time, so the same symbol is a different image under the other appearance.
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

    // Inside the drawing appearance: a semantic tint resolves at raster time, so outside it a
    // `labelColor` symbol picks up the ambient appearance and comes out black on a dark panel.
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

fn size_of_points(points: NSSize) -> Size<Pixels> {
    size(px(points.width as f32), px(points.height as f32))
}

/// Draw an `NSImage` into a fresh premultiplied RGBA bitmap.
///
/// The rep's *size* is set in points while its buffer is in pixels; that mismatch is what gives the
/// 2x transform, and without it the glyph rasterises into the top-left quarter.
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
/// Swapping and stopping there is the trap. Interiors come out byte-identical — dividing by
/// `a = 255` is the identity — so only antialiased edges differ, by `c * a * (1 - a)`, which reads
/// as a dark fringe around every glyph. A black template symbol cannot detect it at all.
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
