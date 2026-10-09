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

#[derive(Clone, Copy)]
pub(crate) enum Sound {
    Trash,
    Done,
    EmptyTrash,
}

impl Sound {
    pub(crate) fn of(op: &Op, undoing: bool) -> Option<Sound> {
        match op {
            Op::Trash(_) if !undoing => Some(Sound::Trash),
            Op::Trash(_)
            | Op::Move { .. }
            | Op::Copy { .. }
            | Op::CopyExactly { .. }
            | Op::Duplicate(_)
            | Op::Compress(_)
            | Op::PutBack(_)
            | Op::Retry(_) => Some(Sound::Done),
            Op::Batch(ops) => Sound::of(ops.first()?, undoing),
            _ => None,
        }
    }

    pub(crate) fn play(self) {
        if let Some(id) = *IDS[self as usize] {
            unsafe { AudioServicesPlaySystemSound(id) };
        }
    }
}
