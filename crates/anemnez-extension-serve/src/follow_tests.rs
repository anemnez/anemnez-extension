//! The follow machine's rows, by sim (D m/32441): a path whose file is written, replaced by a rename over it, and
//! deleted and recreated, each delivered as a bump before the beat where a watch can see it.

use super::*;

/// A filesystem of one path, its files by inode, and a watch bound to the inode it opened, told only what its mask
/// asks (as kqueue's EVFILT_VNODE is).
struct Sim {
    follow: Follow,
    next_inode: u64,
    at_path: Option<u64>,
    watch: Option<(u64, Notes)>,
    bumps: usize,
}

impl Sim {
    fn new() -> Sim {
        let (follow, first) = Follow::start();
        let mut sim = Sim { follow, next_inode: 1, at_path: Some(0), watch: None, bumps: 0 };
        sim.act(vec![first]);
        sim
    }

    fn act(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Bump => self.bumps += 1,
                Action::Open => {
                    self.watch = self.at_path.map(|inode| (inode, WATCHED));
                    let input = if self.watch.is_some() { Input::Opened } else { Input::OpenFailed };
                    let next = self.follow.on(input);
                    self.act(next);
                }
            }
        }
    }

    /// `notes` happened to `inode`: the watch hears them if it's on that inode, within its mask.
    fn to(&mut self, inode: u64, notes: Notes) {
        let Some((watched, mask)) = self.watch else { return };
        let heard = notes.within(mask);
        if watched == inode && heard != Notes::default() {
            let next = self.follow.on(Input::Notes(heard));
            self.act(next);
        }
    }

    fn fresh(&mut self) -> u64 {
        self.next_inode += 1;
        self.next_inode
    }

    fn write(&mut self) {
        if let Some(inode) = self.at_path {
            self.to(inode, Notes { write: true, extend: true, ..Notes::default() });
        }
    }

    /// An atomic replace: a new file written beside, then renamed over the path. The old file's last link goes, so
    /// its watch hears delete; the new file's rename is to a file nobody watches yet.
    fn replace(&mut self) {
        let new = self.fresh();
        let old = self.at_path.replace(new);
        self.to(new, Notes { rename: true, ..Notes::default() });
        if let Some(old) = old {
            self.to(old, Notes { delete: true, ..Notes::default() });
        }
    }

    fn delete(&mut self) {
        if let Some(old) = self.at_path.take() {
            self.to(old, Notes { delete: true, ..Notes::default() });
        }
    }

    fn create(&mut self) {
        self.at_path = Some(self.fresh());
    }

    fn beat(&mut self) {
        let next = self.follow.on(Input::Beat);
        self.act(next);
    }

    /// Bumps since the last ask.
    fn woke(&mut self) -> usize {
        std::mem::take(&mut self.bumps)
    }
}

/// D's row: a write wakes a reader at once; an atomic replace wakes it and the watch follows to the new file, so the
/// new file's next write wakes it too; all before any beat.
#[test]
fn a_write_and_a_replace_each_wake_a_reader_before_the_beat_and_the_watch_follows() {
    let mut sim = Sim::new();
    assert_eq!(sim.follow.state(), State::Watching);
    assert_eq!(sim.woke(), 1, "the first open finds a file");
    sim.write();
    assert_eq!(sim.woke(), 1, "a write wakes once");
    sim.replace();
    assert!(sim.woke() >= 1, "a replace wakes, no beat needed");
    assert_eq!(sim.follow.state(), State::Watching, "watching again");
    assert_eq!(sim.watch.map(|(inode, _)| inode), sim.at_path, "on the new file, not the replaced one");
    sim.write();
    assert_eq!(sim.woke(), 1, "the new file's write wakes: the watch followed it");
}

/// D's row: a delete wakes a reader at once and leaves the path Absent; a file created there is found at the next
/// beat (an event, never a busy loop), and its writes wake a reader after.
#[test]
fn a_delete_wakes_at_once_and_a_recreated_file_is_found_at_the_beat() {
    let mut sim = Sim::new();
    sim.woke();
    sim.delete();
    assert_eq!(sim.woke(), 1, "the delete wakes");
    assert_eq!(sim.follow.state(), State::Absent);
    sim.create();
    sim.write();
    assert_eq!(sim.woke(), 0, "nothing watched while absent: the beat is the bound");
    sim.beat();
    assert_eq!(sim.follow.state(), State::Watching);
    assert_eq!(sim.woke(), 1, "the beat found the new file");
    sim.write();
    assert_eq!(sim.woke(), 1);
}

/// Absent re-tries only at a beat, and a beat with nothing there leaves it Absent and wakes nobody; a beat while
/// watching, or notes from a file no longer watched, change nothing.
#[test]
fn beats_and_stale_notes_wake_nobody() {
    let mut sim = Sim::new();
    sim.woke();
    sim.beat();
    assert_eq!((sim.follow.state(), sim.woke()), (State::Watching, 0), "a beat while watching");
    sim.delete();
    sim.woke();
    sim.beat();
    assert_eq!((sim.follow.state(), sim.woke()), (State::Absent, 0), "still nothing there");
    let mut follow = Follow { state: State::Reopening };
    assert!(follow.on(Input::Notes(Notes { write: true, ..Notes::default() })).is_empty(), "notes while reopening");
}
