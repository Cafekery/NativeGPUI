use std::ffi::c_void;
use std::sync::LazyLock;

use finder_fs::ops::Op;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL};

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioServicesCreateSystemSoundID(url: *const c_void, id: *mut u32) -> i32;
    fn AudioServicesPlaySystemSound(id: u32);
}

static IDS: [LazyLock<Option<u32>>; 3] = [
    LazyLock::new(|| load("dock/drag to trash.aif")),
    LazyLock::new(|| load("system/Volume Mount.aif")),
    LazyLock::new(|| load("finder/empty trash.aif")),
];

fn load(file: &str) -> Option<u32> {
    let path = format!(
        "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/{file}"
    );
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
    let mut id = 0;
    let made = unsafe { AudioServicesCreateSystemSoundID(Retained::as_ptr(&url).cast(), &mut id) };
    (made == 0).then_some(id)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Sound {
    Trash,
    Done,
    EmptyTrash,
}

impl Sound {
    /// What Finder plays when `op` finishes, or when its undo does.
    pub(crate) fn of(op: &Op, undoing: bool) -> Option<Sound> {
        let sound = match op {
            Op::Trash(_) => Sound::Trash,
            Op::Move { .. }
            | Op::Copy { .. }
            | Op::Duplicate(_)
            | Op::Compress(_)
            | Op::PutBack(_) => Sound::Done,
            Op::Batch(ops) => return ops.first().and_then(|op| Sound::of(op, undoing)),
            _ => return None,
        };
        Some(if undoing { Sound::Done } else { sound })
    }

    pub(crate) fn play(self) {
        if let Some(id) = *IDS[self as usize] {
            unsafe { AudioServicesPlaySystemSound(id) };
        }
    }
}
