//! Notices when the window stops answering, and says so in the log.
//!
//! A frozen window leaves nothing behind: the log is silent while the thread that
//! draws is blocked, and Windows only reports the hang once it has already closed
//! the program. So a thread of its own watches how long the current frame has been
//! running. Past [`STALL`] it writes a line while the frame is still in progress,
//! which is when the log can still say something useful. The line names the last
//! section of the frame that finished (see [`section_done`]), so the next freeze
//! points at the code that was running after it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a frame may run before it is reported.
const STALL: Duration = Duration::from_secs(2);

/// Where the frame in progress started, as milliseconds since start plus one; zero
/// between frames.
static FRAME_START: AtomicU64 = AtomicU64::new(0);

/// The name of the last section of the frame to finish.
static LAST_SECTION: Mutex<&'static str> = Mutex::new("frame start");

fn started() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Milliseconds since the watchdog started, plus one so a real reading is never zero.
fn now_ms() -> u64 {
    started().elapsed().as_millis() as u64 + 1
}

fn last_section() -> &'static str {
    LAST_SECTION.lock().map(|g| *g).unwrap_or("unknown")
}

/// Marks the start of a frame.
pub fn frame_begin() {
    set_section("frame start");
    FRAME_START.store(now_ms(), Ordering::Release);
}

/// Marks the end of a frame, and logs it when it took long enough to matter.
pub fn frame_end() {
    let start = FRAME_START.swap(0, Ordering::AcqRel);
    if start == 0 {
        return;
    }
    let took = now_ms().saturating_sub(start);
    if Duration::from_millis(took) >= STALL {
        log::warn!(
            "a frame took {took} ms (last section done: {})",
            last_section()
        );
    }
}

/// Records that a named section of the frame has just finished.
pub fn section_done(name: &'static str) {
    set_section(name);
}

fn set_section(name: &'static str) {
    if let Ok(mut g) = LAST_SECTION.lock() {
        *g = name;
    }
}

/// Starts the thread that reports a frame still running after [`STALL`]. It
/// reports each frame once, and runs for as long as the program does.
pub fn start() {
    started();
    let _ = std::thread::Builder::new()
        .name("rhumb-watchdog".into())
        .spawn(|| {
            let mut reported = 0u64;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let start = FRAME_START.load(Ordering::Acquire);
                if start == 0 || start == reported {
                    continue;
                }
                let running = now_ms().saturating_sub(start);
                if Duration::from_millis(running) >= STALL {
                    log::error!(
                        "the window has not finished a frame for {running} ms (last section done: {})",
                        last_section()
                    );
                    reported = start;
                }
            }
        });
}
