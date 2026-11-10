use std::cell::{Cell, RefCell};
use std::rc::Rc;

use finder_fs::kind::Category;
use gpui::{AnyElement, Context, canvas, prelude::*, px};
use native_gpui::{Layer, NativeDispatch, NativeHost, NativeRead, wire};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSFont, NSResponder, NSRuleEditor,
    NSRuleEditorDelegate, NSRuleEditorNestingMode, NSRuleEditorRowType, NSTextField,
    NSTextFieldDelegate, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSInteger, NSNotification, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSize, NSString, ns_string,
};

use crate::app::FinderDemo;
use crate::data::Row;

const TESTS: [&str; 4] = ["contains", "begins with", "ends with", "is"];
const KEY: u64 = u64::from_be_bytes(*b"criteria");

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Criterion {
    by_name: bool,
    kind: Category,
    test: usize,
    text: String,
}

impl Criterion {
    pub(crate) fn name() -> Self {
        Self {
            by_name: true,
            ..Self::default()
        }
    }

    pub(crate) fn holds(&self, row: &Row) -> bool {
        if !self.by_name {
            return match self.kind {
                Category::Other => row.kind.to_lowercase().contains(&self.text.to_lowercase()),
                kind => finder_fs::kind::conforms(&row.uti, kind),
            };
        }
        let name = row.name.to_lowercase();
        let wanted = self.text.to_lowercase();
        match self.test() {
            "begins with" => name.starts_with(&wanted),
            "ends with" => name.ends_with(&wanted),
            "is" => name == wanted,
            _ => name.contains(&wanted),
        }
    }

    fn test(&self) -> &'static str {
        TESTS[self.test]
    }

    pub(crate) fn raw_query(&self) -> Option<String> {
        if self.by_name {
            let (before, after) = match self.test() {
                "begins with" => ("", "*"),
                "ends with" => ("*", ""),
                "is" => ("", ""),
                _ => ("*", "*"),
            };
            return (!self.text.is_empty())
                .then(|| crate::smart::name_clause(before, &self.text, after));
        }
        match self.kind {
            Category::Any => None,
            Category::Other => (!self.text.is_empty()).then(|| {
                format!(
                    "(kMDItemKind == \"*{}*\"cd)",
                    crate::smart::escape(&self.text)
                )
            }),
            kind => kind
                .identifier()
                .map(|uti| format!("(kMDItemContentTypeTree == \"{uti}\")")),
        }
    }

    fn path(&self) -> Vec<Node> {
        if self.by_name {
            return vec![Node::Name, Node::Test(self.test), Node::Text];
        }
        let kind = Category::ALL.iter().position(|it| *it == self.kind);
        let mut path = vec![Node::Kind, Node::Is, Node::Category(kind.unwrap_or(0))];
        if self.kind == Category::Other {
            path.push(Node::Text);
        }
        path
    }
}

#[derive(Clone, Copy)]
enum Node {
    Root,
    Kind,
    Name,
    Is,
    Category(usize),
    Test(usize),
    Text,
}

impl Node {
    fn of(criterion: Option<&AnyObject>) -> Self {
        let Some(name) = criterion.and_then(|it| it.downcast_ref::<NSString>()) else {
            return Node::Root;
        };
        let name = name.to_string();
        let at = |prefix: &str| name.strip_prefix(prefix)?.parse().ok();
        match name.as_str() {
            "kind" => Node::Kind,
            "name" => Node::Name,
            "is" => Node::Is,
            _ => at("kind:")
                .map(Node::Category)
                .or_else(|| at("test:").map(Node::Test))
                .unwrap_or(Node::Text),
        }
    }

    fn name(self) -> Retained<NSString> {
        match self {
            Node::Kind => ns_string!("kind").retain(),
            Node::Name => ns_string!("name").retain(),
            Node::Is => ns_string!("is").retain(),
            Node::Category(at) => NSString::from_str(&format!("kind:{at}")),
            Node::Test(at) => NSString::from_str(&format!("test:{at}")),
            Node::Root | Node::Text => ns_string!("text").retain(),
        }
    }

    fn children(self) -> Vec<Node> {
        match self {
            Node::Root => vec![Node::Kind, Node::Name],
            Node::Kind => vec![Node::Is],
            Node::Is => (0..Category::ALL.len()).map(Node::Category).collect(),
            Node::Category(at) if Category::ALL[at] == Category::Other => vec![Node::Text],
            Node::Name => (0..TESTS.len()).map(Node::Test).collect(),
            Node::Test(_) => vec![Node::Text],
            _ => Vec::new(),
        }
    }

    fn title(self) -> &'static str {
        match self {
            Node::Kind => "Kind",
            Node::Name => "Name",
            Node::Is => "is",
            Node::Category(at) => Category::ALL[at].label(),
            Node::Test(at) => TESTS[at],
            Node::Root | Node::Text => "",
        }
    }
}

#[derive(Default)]
struct EditorIvars {
    pending: Cell<usize>,
    written: RefCell<Vec<Criterion>>,
}

define_class!(
    #[unsafe(super(NSRuleEditor, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGCriteriaEditor"]
    #[ivars = EditorIvars]
    struct Editor;

    unsafe impl NSObjectProtocol for Editor {}

    unsafe impl NSRuleEditorDelegate for Editor {
        #[unsafe(method(ruleEditor:numberOfChildrenForCriterion:withRowType:))]
        fn child_count(
            &self,
            _: &NSRuleEditor,
            criterion: Option<&AnyObject>,
            _: NSRuleEditorRowType,
        ) -> NSInteger {
            Node::of(criterion).children().len() as NSInteger
        }

        #[unsafe(method_id(ruleEditor:child:forCriterion:withRowType:))]
        fn child(
            &self,
            _: &NSRuleEditor,
            index: NSInteger,
            criterion: Option<&AnyObject>,
            _: NSRuleEditorRowType,
        ) -> Retained<AnyObject> {
            Node::of(criterion).children()[index as usize].name().into()
        }

        #[unsafe(method_id(ruleEditor:displayValueForCriterion:inRow:))]
        fn display_value(
            &self,
            _: &NSRuleEditor,
            criterion: &AnyObject,
            row: NSInteger,
        ) -> Retained<AnyObject> {
            self.value(Node::of(Some(criterion)), row)
        }
    }

    unsafe impl NSControlTextEditingDelegate for Editor {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _: &NSNotification) {
            unsafe { self.sendAction_to(self.action(), self.target().as_deref()) };
        }
    }

    unsafe impl NSTextFieldDelegate for Editor {}
);

impl Editor {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(EditorIvars::default());
        let editor: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        editor.setDelegate(Some(ProtocolObject::from_ref(&*editor)));
        editor.setNestingMode(NSRuleEditorNestingMode::List);
        editor.setCanRemoveAllRows(true);
        editor
    }

    fn field(&self, row: NSInteger) -> Option<Retained<NSTextField>> {
        (row < self.numberOfRows())
            .then(|| self.displayValuesForRow(row))?
            .iter()
            .find_map(|value| value.downcast::<NSTextField>().ok())
    }

    /// A row keeps its field, and so its text, across a change of operator.
    fn value(&self, node: Node, row: NSInteger) -> Retained<AnyObject> {
        if !matches!(node, Node::Text) {
            return NSString::from_str(node.title()).into();
        }
        let field = self.field(row).unwrap_or_else(|| {
            let frame = NSRect::new(NSPoint::ZERO, NSSize::new(250., 19.));
            let field = NSTextField::initWithFrame(NSTextField::alloc(self.mtm()), frame);
            field.setFont(Some(&NSFont::controlContentFontOfSize(11.)));
            unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
            field
        });
        field.into()
    }

    fn criterion(&self, row: NSInteger) -> Criterion {
        let mut criterion = Criterion::default();
        for node in self.criteriaForRow(row).iter() {
            match Node::of(Some(&node)) {
                Node::Name => criterion.by_name = true,
                Node::Category(at) => criterion.kind = Category::ALL[at],
                Node::Test(at) => criterion.test = at,
                _ => {}
            }
        }
        if let Some(field) = self.field(row) {
            criterion.text = field.stringValue().to_string();
        }
        criterion
    }

    fn write(&self, rows: &[Criterion]) {
        let (have, want) = (self.numberOfRows(), rows.len() as NSInteger);
        for row in (want..have).rev() {
            self.removeRowAtIndex(row);
        }
        for row in have..want {
            self.insertRowAtIndex_withType_asSubrowOfRow_animate(
                row,
                NSRuleEditorRowType::Simple,
                -1,
                false,
            );
        }
        for (row, criterion) in (0..).zip(rows) {
            let path = criterion.path();
            let criteria: Vec<Retained<AnyObject>> =
                path.iter().map(|node| node.name().into()).collect();
            let values: Vec<Retained<AnyObject>> =
                path.iter().map(|node| self.value(*node, row)).collect();
            if let Some(field) = values
                .iter()
                .find_map(|it| it.downcast_ref::<NSTextField>())
            {
                field.setStringValue(&NSString::from_str(&criterion.text));
            }
            unsafe {
                self.setCriteria_andDisplayValues_forRowAtIndex(
                    &NSArray::from_retained_slice(&criteria),
                    &NSArray::from_retained_slice(&values),
                    row,
                )
            };
        }
        self.ivars().written.replace(rows.to_vec());
    }
}

/// Bumps `pending` until the deferred handler lands: a paint in between must not rewrite the rows.
fn read() -> NativeRead {
    thread_local! {
        static READ: NativeRead = Rc::new(|sender| {
            let Some(editor) = sender.downcast_ref::<Editor>() else {
                return Box::new(());
            };
            let rows: Vec<Criterion> = (0..editor.numberOfRows())
                .map(|row| editor.criterion(row))
                .collect();
            let ivars = editor.ivars();
            ivars.pending.set(ivars.pending.get() + 1);
            ivars.written.replace(rows.clone());
            Box::new((rows, editor.retain()))
        });
    }
    READ.with(Rc::clone)
}

pub(crate) fn row_height() -> f32 {
    native_gpui::fitting_size("NSRuleEditor.rowHeight", |mtm| {
        NSSize::new(0., NSRuleEditor::new(mtm).rowHeight())
    })
    .height as f32
}

impl FinderDemo {
    pub(crate) fn criteria_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.criteria.is_empty() {
            return None;
        }
        let rows = self.criteria.clone();
        let height = row_height() * rows.len() as f32;
        let demo = cx.entity().downgrade();
        let dispatch: NativeDispatch = Rc::new(move |value, _, cx| {
            let Ok(value) = value.downcast::<(Vec<Criterion>, Retained<Editor>)>() else {
                return;
            };
            let (rows, editor) = *value;
            let pending = &editor.ivars().pending;
            pending.set(pending.get().saturating_sub(1));
            demo.update(cx, |this, cx| {
                this.criteria = rows;
                this.run_search(cx);
            })
            .ok();
        });
        let paint = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let host = NativeHost::get(window, cx);
                let mask = window.content_mask().bounds;
                let editor = host.place(KEY, Layer::Above, bounds, mask, |mtm, target| {
                    let editor = Editor::new(mtm);
                    wire(&editor, target, KEY);
                    editor
                });
                let ivars = editor.ivars();
                if ivars.pending.get() == 0 && *ivars.written.borrow() != rows {
                    editor.write(&rows);
                }
                host.on_event(KEY, read(), Some(dispatch));
            },
        );
        Some(paint.w_full().h(px(height)).into_any_element())
    }
}
