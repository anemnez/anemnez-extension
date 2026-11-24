//! Following a log's file through its replaces, pure (rule 1; D m/32441, L m/32438): what bumps a per-read log's
//! [`Changes`](crate::reread::Changes), so a Watch on it wakes when the file moves, never only at its beat.
//!
//! The host watches one open file (kqueue's EVFILT_VNODE on macOS) and hands this machine what it saw. A file that's
//! written or extended is a change. One that's deleted, renamed or revoked is no longer the path's file: the machine
//! re-opens the path, so after an atomic replace (a rename over it) it follows the new file. A path that isn't there is
//! Absent, re-tried at each beat: the beat is an event the host hands in, never a busy loop and never a hang (rule 4).
//!
//! ```text
//!             Opened                     Notes: delete | rename | revoke
//! Reopening ─────────▶ Watching ─────────────────────────────────▶ Reopening
//!     │  ▲                │ Notes: write | extend → bump, stay
//!     │  │ Beat           ▼
//!     └─▶ Absent ◀── OpenFailed
//! ```
//!
//! Invariants: it bumps once whenever the path's file may hold something new (it was written or extended, it stopped
//! being the path's file, or a file was found at the path), and at no other time, so a re-read after a replace finds
//! the new file and nothing else wakes a reader. The machine asks for every note it acts on ([`WATCHED`]): a host that
//! watched only writes would never see a replace.

/// What a watch on one open file saw, as kqueue's vnode notes say it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    pub write: bool,
    pub extend: bool,
    pub delete: bool,
    pub rename: bool,
    pub revoke: bool,
}

impl Notes {
    /// Only the notes `self` and `mask` both have: what a host watching `mask` is told.
    #[must_use]
    pub fn within(self, mask: Notes) -> Notes {
        Notes {
            write: self.write && mask.write,
            extend: self.extend && mask.extend,
            delete: self.delete && mask.delete,
            rename: self.rename && mask.rename,
            revoke: self.revoke && mask.revoke,
        }
    }

    fn gone(self) -> bool {
        self.delete || self.rename || self.revoke
    }

    fn moved(self) -> bool {
        self.write || self.extend
    }
}

/// Every note the machine acts on: the host watches these, and no fewer.
pub const WATCHED: Notes = Notes { write: true, extend: true, delete: true, rename: true, revoke: true };

/// Where the machine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Opening the path, the open asked of the host.
    Reopening,
    /// Watching the path's file, opened.
    Watching,
    /// The path isn't there: re-tried at the next beat.
    Absent,
}

/// What the host hands in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The path opened, and its watch is armed with [`WATCHED`].
    Opened,
    /// The path didn't open (it isn't there, or can't be read).
    OpenFailed,
    /// The watched file's notes.
    Notes(Notes),
    /// A beat passed with nothing else to say.
    Beat,
}

/// What the machine asks of the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Bump the log's changes: every wait past the old generation wakes.
    Bump,
    /// Close any watch, then open the path and arm a watch for [`WATCHED`].
    Open,
}

/// The follow machine.
#[derive(Clone, Debug)]
pub struct Follow {
    state: State,
}

impl Follow {
    /// A machine that starts by opening the path, and the action that asks it.
    #[must_use]
    pub fn start() -> (Follow, Action) {
        (Follow { state: State::Reopening }, Action::Open)
    }

    /// Where it is now.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Take one input; what to do, in order.
    pub fn on(&mut self, input: Input) -> Vec<Action> {
        match (self.state, input) {
            (State::Reopening, Input::Opened) => {
                // A new file at the path, maybe after a replace: one bump, so a re-read finds it.
                self.state = State::Watching;
                vec![Action::Bump]
            }
            (State::Reopening, Input::OpenFailed) => {
                // Nothing at the path: nothing new to read, until a beat finds a file.
                self.state = State::Absent;
                Vec::new()
            }
            (State::Absent, Input::Beat) => {
                self.state = State::Reopening;
                vec![Action::Open]
            }
            (State::Watching, Input::Notes(notes)) if notes.gone() => {
                // Not the path's file any more: one bump for the transition (a write in the same notes is in it), then
                // the path again.
                self.state = State::Reopening;
                vec![Action::Bump, Action::Open]
            }
            (State::Watching, Input::Notes(notes)) if notes.moved() => vec![Action::Bump],
            // Notes from a file no longer watched, a beat while watching or opening, or an open answered twice: nothing
            // moved.
            _ => Vec::new(),
        }
    }
}

/// The refusal of [`crate::reread::Changes::follow`] where there's no host for it: a Watch on the log then wakes at
/// its beat alone, as before.
pub const FOLLOW_UNSUPPORTED: &str = "follow-unsupported";

/// A running follow: dropping it stops the watch within a beat.
pub struct Following {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Following {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Follow `path` on a thread of its own, bumping `changes` as the machine says, waking at least every `beat`.
pub(crate) fn follow(
    changes: std::sync::Arc<crate::reread::Changes>,
    path: &std::path::Path,
    beat: std::time::Duration,
) -> Result<Following, String> {
    host::spawn(changes, path, beat)
}

/// The thin host: kqueue's EVFILT_VNODE on the open file, macOS first (D m/32441).
#[cfg(target_os = "macos")]
mod host {
    use std::os::unix::ffi::OsStrExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{Action, Follow, Following, Input, Notes, WATCHED};
    use crate::reread::Changes;

    fn fflags(notes: Notes) -> u32 {
        let mut f = 0;
        if notes.write {
            f |= libc::NOTE_WRITE;
        }
        if notes.extend {
            f |= libc::NOTE_EXTEND;
        }
        if notes.delete {
            f |= libc::NOTE_DELETE;
        }
        if notes.rename {
            f |= libc::NOTE_RENAME;
        }
        if notes.revoke {
            f |= libc::NOTE_REVOKE;
        }
        f
    }

    fn notes(f: u32) -> Notes {
        Notes {
            write: f & libc::NOTE_WRITE != 0,
            extend: f & libc::NOTE_EXTEND != 0,
            delete: f & libc::NOTE_DELETE != 0,
            rename: f & libc::NOTE_RENAME != 0,
            revoke: f & libc::NOTE_REVOKE != 0,
        }
    }

    fn event(fd: i32, flags: u16, fflags: u32) -> libc::kevent {
        libc::kevent {
            ident: usize::try_from(fd).unwrap_or(0),
            filter: libc::EVFILT_VNODE,
            flags,
            fflags,
            data: 0,
            udata: std::ptr::null_mut(),
        }
    }

    /// One open file and its watch: closed when dropped or replaced.
    struct Watched {
        kq: i32,
        fd: i32,
    }

    impl Watched {
        /// Open `path` for events only and arm its watch for [`WATCHED`]; false when it isn't there or won't arm.
        fn open(&mut self, path: &std::ffi::CString) -> bool {
            self.close();
            // SAFETY: path is a valid NUL-terminated string; O_EVTONLY opens for events, never data.
            let fd = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                return false;
            }
            let arm = event(fd, libc::EV_ADD | libc::EV_CLEAR, fflags(WATCHED));
            // SAFETY: kq and fd are open descriptors this host owns; one change, no events read back.
            let armed = unsafe { libc::kevent(self.kq, &raw const arm, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
            if armed < 0 {
                // SAFETY: fd was opened above and is closed once.
                unsafe { libc::close(fd) };
                return false;
            }
            self.fd = fd;
            true
        }

        fn close(&mut self) {
            if self.fd >= 0 {
                // SAFETY: fd is open and owned here; closing it also removes its kevent.
                unsafe { libc::close(self.fd) };
                self.fd = -1;
            }
        }

        /// Wait at most `beat` for the watch's notes: None when the beat passed.
        fn wait(&self, beat: Duration) -> Option<Notes> {
            let timeout = libc::timespec {
                tv_sec: libc::time_t::try_from(beat.as_secs()).unwrap_or(libc::time_t::MAX),
                tv_nsec: libc::c_long::from(beat.subsec_nanos()),
            };
            let mut got = event(-1, 0, 0);
            // SAFETY: kq is open; got has room for the one event asked; the wait is bounded by timeout.
            let n = unsafe { libc::kevent(self.kq, std::ptr::null(), 0, &raw mut got, 1, &raw const timeout) };
            (n > 0 && got.filter == libc::EVFILT_VNODE).then(|| notes(got.fflags))
        }
    }

    impl Drop for Watched {
        fn drop(&mut self) {
            self.close();
            // SAFETY: kq is open and owned here, closed once.
            unsafe { libc::close(self.kq) };
        }
    }

    pub(super) fn spawn(changes: Arc<Changes>, path: &std::path::Path, beat: Duration) -> Result<Following, String> {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("{}: a NUL in the path", path.display()))?;
        // SAFETY: kqueue takes no arguments and returns a new descriptor or -1.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(format!("kqueue: {}", std::io::Error::last_os_error()));
        }
        let mut watched = Watched { kq, fd: -1 };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("events-follow".into())
            .spawn(move || {
                let (mut machine, first) = Follow::start();
                let mut asked = vec![first];
                while !stopped.load(Ordering::SeqCst) {
                    while let Some(action) = (!asked.is_empty()).then(|| asked.remove(0)) {
                        match action {
                            Action::Bump => changes.bump(),
                            Action::Open => {
                                let input = if watched.open(&path) { Input::Opened } else { Input::OpenFailed };
                                asked.extend(machine.on(input));
                            }
                        }
                    }
                    let input = watched.wait(beat).map_or(Input::Beat, Input::Notes);
                    asked.extend(machine.on(input));
                }
            })
            .map_err(|e| format!("the follow thread: {e}"))?;
        Ok(Following { stop })
    }
}

#[cfg(not(target_os = "macos"))]
mod host {
    pub(super) fn spawn(
        _changes: std::sync::Arc<crate::reread::Changes>,
        _path: &std::path::Path,
        _beat: std::time::Duration,
    ) -> Result<super::Following, String> {
        Err(format!("{}: no file watch on this host yet; a Watch wakes at its beat", super::FOLLOW_UNSUPPORTED))
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
