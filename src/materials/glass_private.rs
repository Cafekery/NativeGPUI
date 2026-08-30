//! The undocumented `NSGlassEffectView` surface, behind the `private-api` feature.
//!
//! Every selector here is resolved at runtime, so a macOS that drops one degrades to the public
//! behaviour rather than raising. Only knobs measured to move pixels are exposed:
//! `_adaptiveAppearance`, `_groupIdentifier`, `_useReducedShadowRadius` and `clipsToBounds`
//! change nothing at any value, and `_vibrantBlendingStyleForSubtree` no longer exists.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::sel;
use objc2_app_kit::NSGlassEffectView;
use objc2_foundation::NSString;

use super::glass::glass_style;
use super::private_api::{read_isize, read_string, send_isize, send_string};

/// The raw `_variant` the public `style` resolves to, which is how walking back off an explicit
/// variant is detected: `style` reports Regular for every override, so it cannot tell you.
const STYLE_REGULAR: isize = 1;
const STYLE_CLEAR: isize = 2;

/// The internal recipe the glass draws, one entry per render class that draws distinctly. Named
/// by the lowest raw value in its class; raw 1 draws as [`GlassVariant::Automatic`].
///
/// Two are near-invisible over content in the same window: [`GlassVariant::Keyboard`] draws
/// nothing at all, and [`GlassVariant::Sidebar`] draws only within about 24pt of the panel edge.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GlassVariant {
    Automatic,
    Clear,
    Dock,
    AppIcons,
    Widgets,
    AvPlayer,
    NotificationCenter,
    Monogram,
    Bubbles,
    Keyboard,
    Sidebar,
    Slider,
    CartouchePopover,
}

impl GlassVariant {
    pub const ALL: &'static [Self] = &[
        Self::Automatic,
        Self::Clear,
        Self::Dock,
        Self::AppIcons,
        Self::Widgets,
        Self::AvPlayer,
        Self::NotificationCenter,
        Self::Monogram,
        Self::Bubbles,
        Self::Keyboard,
        Self::Sidebar,
        Self::Slider,
        Self::CartouchePopover,
    ];

    pub fn raw(self) -> isize {
        match self {
            Self::Automatic => 0,
            Self::Clear => 2,
            Self::Dock => 3,
            Self::AppIcons => 4,
            Self::Widgets => 5,
            Self::AvPlayer => 6,
            Self::NotificationCenter => 9,
            Self::Monogram => 10,
            Self::Bubbles => 11,
            Self::Keyboard => 13,
            Self::Sidebar => 14,
            Self::Slider => 19,
            Self::CartouchePopover => 20,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Clear => "Clear",
            Self::Dock => "Dock",
            Self::AppIcons => "App Icons",
            Self::Widgets => "Widgets",
            Self::AvPlayer => "AV Player",
            Self::NotificationCenter => "Notification Centre",
            Self::Monogram => "Monogram",
            Self::Bubbles => "Bubbles",
            Self::Keyboard => "Keyboard",
            Self::Sidebar => "Sidebar",
            Self::Slider => "Slider",
            Self::CartouchePopover => "Cartouche Popover",
        }
    }

    /// The subvariants that draw under this variant. An explicit variant writes a whole recipe
    /// and a subvariant only patches what it leaves open, so most go inert.
    pub fn subvariants(self) -> &'static [GlassSubvariant] {
        use GlassSubvariant as S;
        const HOMESCREEN: &[GlassSubvariant] =
            &[S::None, S::HomescreenResizeHandle, S::HomescreenClose];
        match self {
            Self::Automatic => GlassSubvariant::ALL,
            Self::AppIcons | Self::Widgets | Self::Keyboard | Self::Sidebar => &[S::None],
            Self::Monogram => &[
                S::None,
                S::Camera,
                S::HomescreenResizeHandle,
                S::HomescreenClose,
            ],
            Self::Bubbles => &[
                S::None,
                S::EntryField,
                S::HomescreenResizeHandle,
                S::HomescreenClose,
            ],
            Self::Clear
            | Self::Dock
            | Self::AvPlayer
            | Self::NotificationCenter
            | Self::Slider
            | Self::CartouchePopover => HOMESCREEN,
        }
    }

    pub fn accepts(self, subvariant: GlassSubvariant) -> bool {
        self.subvariants().contains(&subvariant)
    }
}

/// The named refinement layered on the variant. The property is an open string enum, so the
/// names outside this set are accepted and draw nothing.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum GlassSubvariant {
    #[default]
    None,
    EntryField,
    Camera,
    Track,
    SiriSnippet,
    IosNotificationCenter,
    Menu,
    Tab,
    HomescreenResizeHandle,
    HomescreenClose,
}

impl GlassSubvariant {
    pub const ALL: &'static [Self] = &[
        Self::None,
        Self::EntryField,
        Self::Camera,
        Self::Track,
        Self::SiriSnippet,
        Self::IosNotificationCenter,
        Self::Menu,
        Self::Tab,
        Self::HomescreenResizeHandle,
        Self::HomescreenClose,
    ];

    pub fn api_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::EntryField => Some("entryField"),
            Self::Camera => Some("camera"),
            Self::Track => Some("track"),
            Self::SiriSnippet => Some("siriSnippet"),
            Self::IosNotificationCenter => Some("iOSNotificationCenter"),
            Self::Menu => Some("menu"),
            Self::Tab => Some("tab"),
            Self::HomescreenResizeHandle => Some("homescreenResizeHandle"),
            Self::HomescreenClose => Some("homescreenClose"),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::EntryField => "Entry Field",
            Self::Camera => "Camera",
            Self::Track => "Track",
            Self::SiriSnippet => "Siri Snippet",
            Self::IosNotificationCenter => "iOS Notification Centre",
            Self::Menu => "Menu",
            Self::Tab => "Tab",
            Self::HomescreenResizeHandle => "Homescreen Resize Handle",
            Self::HomescreenClose => "Homescreen Close",
        }
    }
}

/// Hover and pressed glass. Closed rather than an integer: AppKit switches over this
/// exhaustively and anything outside 0...2 traps uncatchably, taking the process with it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum GlassInteractionState {
    #[default]
    None,
    Interactive,
    Pressed,
}

impl GlassInteractionState {
    pub const ALL: &'static [Self] = &[Self::None, Self::Interactive, Self::Pressed];

    pub fn raw(self) -> isize {
        match self {
            Self::None => 0,
            Self::Interactive => 1,
            Self::Pressed => 2,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Interactive => "Interactive",
            Self::Pressed => "Pressed",
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Private {
    pub variant: Option<GlassVariant>,
    pub subvariant: GlassSubvariant,
    pub interaction: GlassInteractionState,
    pub subdued: bool,
    pub scrim: bool,
    pub lensing: bool,
}

impl Private {
    pub(super) fn apply(&self, view: &NSGlassEffectView, clear: bool) {
        let wanted = self.variant.map_or(
            if clear { STYLE_CLEAR } else { STYLE_REGULAR },
            GlassVariant::raw,
        );
        // `style` and `_variant` are one piece of state and the last write wins, so the variant
        // is written only when the caller overrode it — writing it unconditionally leaves the
        // public `clear` dead. The style write is unconditional inside the guard because `style`
        // reads back as Regular under every override and so cannot report that one is in place.
        if read_isize(view, sel!(_variant)) != Some(wanted) {
            view.setStyle(glass_style(clear));
            if let Some(variant) = self.variant {
                send_isize(view, sel!(set_variant:), variant.raw());
            }
        }

        let name = subvariant_name(self.effective_subvariant());
        if read_string(view, sel!(_subvariant)).as_deref() != name.as_deref() {
            send_string(view, sel!(set_subvariant:), name.as_deref());
        }

        // AppKit switches over `_interactionState` exhaustively: outside 0...2 it traps
        // uncatchably and takes the process with it, so the clamp backs up the closed enum.
        set_state(
            view,
            sel!(_interactionState),
            sel!(set_interactionState:),
            self.interaction.raw().clamp(0, 2),
        );
        // Each of these three draws at exactly one measured value and at no other.
        set_state(
            view,
            sel!(_subduedState),
            sel!(set_subduedState:),
            isize::from(self.subdued),
        );
        set_state(
            view,
            sel!(_scrimState),
            sel!(set_scrimState:),
            isize::from(self.scrim),
        );
        set_state(
            view,
            sel!(_contentLensing),
            sel!(set_contentLensing:),
            2 * isize::from(self.lensing),
        );
    }

    pub(super) fn effective_subvariant(&self) -> GlassSubvariant {
        match self.variant {
            Some(variant) if !variant.accepts(self.subvariant) => GlassSubvariant::None,
            _ => self.subvariant,
        }
    }
}

/// Push a value only when the view is not already at it: the diff runs every frame, and a
/// redundant write rebuilds the recipe.
fn set_state(view: &NSGlassEffectView, getter: Sel, setter: Sel, value: isize) {
    if read_isize(view, getter) != Some(value) {
        send_isize(view, setter, value);
    }
}

/// Memoized: the subvariant diff runs every frame and there are nine names.
fn subvariant_name(subvariant: GlassSubvariant) -> Option<Retained<NSString>> {
    thread_local! {
        static CACHE: RefCell<HashMap<&'static str, Retained<NSString>>> =
            RefCell::new(HashMap::new());
    }
    let name = subvariant.api_name()?;
    Some(CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(name)
            .or_insert_with(|| NSString::from_str(name))
            .clone()
    }))
}
