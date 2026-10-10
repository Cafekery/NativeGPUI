use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static PAUSE: AtomicBool = AtomicBool::new(false);
static RESUME: AtomicBool = AtomicBool::new(false);

extern "C" fn caught(signal: libc::c_int) {
    match signal {
        libc::SIGUSR1 => RESUME.store(true, Ordering::SeqCst),
        libc::SIGUSR2 => PAUSE.store(true, Ordering::SeqCst),
        _ => {
            INTERRUPTED.store(true, Ordering::SeqCst);
            // A second Control-C ends the process at once.
            unsafe { libc::signal(signal, libc::SIG_DFL) };
        }
    }
}

/// Control-C and SIGTERM cancel through the engine; SIGUSR2 pauses and SIGUSR1 resumes.
pub fn install() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGUSR1, libc::SIGUSR2] {
        unsafe {
            libc::signal(
                signal,
                caught as extern "C" fn(libc::c_int) as libc::sighandler_t,
            )
        };
    }
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn take_pause() -> bool {
    PAUSE.swap(false, Ordering::SeqCst)
}

pub fn take_resume() -> bool {
    RESUME.swap(false, Ordering::SeqCst)
}
