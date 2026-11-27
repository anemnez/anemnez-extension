//! The cursor face's kernel (CONTRACT.md §2a; agora DELIVERY.md §1a, §1b): what `Cursor/Next` and `Cursor/Commit`
//! decide, pure and sync (house rule 1). The face's host reads the product's log and its cursors, feeds this kernel
//! what it read and the time, and does what it answers: read on, answer, or write one change.
//!
//! - **One mark kernel, never a second** (rule 2). A recipient's marks are hydramq-cursor's [`Marks`]: `Ack` only
//!   rises, `Hand` records what is in flight, and the store fences each write. This kernel only maps the face's
//!   positions onto them ([`marked`], [`commit`]) and adds no mark logic of its own.
//! - **A position is one-based**: the item at the log's offset `o` is at position `o + 1`, so `0` is before the
//!   first item. `acked = p` says every item through `p` is acked, inclusive, as the delivery client reads it
//!   (`Next { after: acked }`), and that is hydramq-cursor's `next` exactly: the first unacked offset is `p`. A fresh
//!   recipient's marks are `0`, and its first `Next` after `0` reads the log from its first item.
//! - **`Next` scans page by page inside the call's bound** ([`Scan`]): the product's `Whose` filters, so a page can
//!   hold nothing of this recipient's. A scan cut by its bound answers what it found and how far it read
//!   (`scanned_to`), never `events-call-unanswered`, so a sparse recipient over a long log is answered every call (E
//!   m/43868). Only an empty answer with `scanned_to` at the head is caught up.
//! - **Below the retained start is `Trimmed`, by name** ([`Answer::Trimmed`]): `from` is the first position trimmed past
//!   the recipient's `after`, `to` the last trimmed, both inclusive (D m/43584), so a client that acks through `to` is
//!   handed the first retained item next. Never a silent jump.
//! - **A commit only rises** ([`commit`]): at or below the mark it is done, answered with the mark as it stands (rule 5:
//!   the client reads "ours or further"); past the log's end it is refused `commit-past-end`, naming the end; an
//!   `acked` past `handed` raises `handed` with it, in the one write (handed is advisory: never refused).
//! - **Only the recipient or a listed product** ([`admits`]): anyone else is `cursor-not-yours`, for both calls.

use anemnez_clock::Mono;
use hydramq_cursor::{Change, Marks};

/// A one-based position in the product's log: the item at offset `o` is at `o + 1`, and `0` is before the first.
pub type Pos = u64;

/// The position of the item at the log's `offset`.
#[must_use]
pub fn position_of(offset: u64) -> Pos {
    offset.saturating_add(1)
}

/// Which of a recipient's two marks a commit moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The newest position a turn ended Ok with: load-bearing.
    Acked,
    /// The newest position handed to a session: advisory, for diagnosis (D's §11 ruling).
    Handed,
}

/// A recipient's marks, as the face says them: `handed` is never below `acked`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marked {
    /// Every item through this position is acked.
    pub acked: Pos,
    /// Every item through this position was handed (at least `acked`).
    pub handed: Pos,
}

/// The face's marks of hydramq-cursor's: `acked` is the first unacked offset (so the last acked position), and
/// `handed` the last handed offset's position, never below `acked`.
#[must_use]
pub fn marked(marks: &Marks) -> Marked {
    let acked = marks.next();
    let handed = marks.handed().map_or(acked, |h| position_of(h).max(acked));
    Marked { acked, handed }
}

/// What a commit decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// At or below the mark: nothing to write, and the answer is the mark as it stands (a repeat is harmless).
    Done(Marked),
    /// Write this one change to the recipient's marks; the answer is the marks it makes.
    Write(Change),
}

/// Why a commit was refused, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitRefused {
    /// `commit-past-end`: `through` is past the log's last position, `end`.
    PastEnd {
        /// The log's last position (`0`: the log holds nothing).
        end: Pos,
    },
}

impl std::fmt::Display for CommitRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PastEnd { end } => write!(
                f,
                "{}: the log ends at {end}; a client is never handed past it, so nothing past it is committed",
                anemnez_extension::refusal::COMMIT_PAST_END
            ),
        }
    }
}

/// Decide a commit of `mark` through `through` on `marks`, whose log's last position is `end`.
///
/// An `acked` commit past `handed` raises `handed` with it in the one write: hydramq-cursor's `Ack` drops a `handed`
/// below the new mark, and [`marked`] says `handed` at least `acked`.
///
/// # Errors
/// [`CommitRefused::PastEnd`] when `through` is past `end`, whatever the marks.
pub fn commit(marks: &Marks, mark: Mark, through: Pos, end: Pos) -> Result<Commit, CommitRefused> {
    if through > end {
        return Err(CommitRefused::PastEnd { end });
    }
    let now = marked(marks);
    match mark {
        Mark::Acked if through <= now.acked => Ok(Commit::Done(now)),
        Mark::Acked => Ok(Commit::Write(Change::Ack { through })),
        Mark::Handed if through <= now.handed => Ok(Commit::Done(now)),
        // `through > handed >= acked >= 0`, so `through - 1` is an offset at or past the first unacked one.
        Mark::Handed => Ok(Commit::Write(Change::Hand { through: through - 1 })),
    }
}

/// Who asks: a carried call's principal, or a product caller the face's host judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller<'a> {
    /// A carried call whose principal is this identity.
    Principal(&'a str),
    /// A product caller, by its code identifier (Agora's, Substrate's), calling unstamped (M m/43783).
    Product(&'a str),
    /// Neither: no principal and no judged product.
    Unknown,
}

/// Why a caller was refused, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotYours {
    /// The recipient asked for.
    pub recipient: String,
    /// Who asked, as said.
    pub caller: String,
}

impl std::fmt::Display for NotYours {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} asked for {}'s marks; only {} itself or a product this face lists reads or moves them",
            anemnez_extension::refusal::CURSOR_NOT_YOURS,
            self.caller,
            self.recipient,
            self.recipient
        )
    }
}

/// Whether `caller` may read or move `recipient`'s marks: the recipient itself (names compared as hydramq-cursors
/// keys them, trimmed and case-folded), or a product on `products`, the face's `callers = PRODUCT` list.
///
/// # Errors
/// [`NotYours`] (`cursor-not-yours`) for anyone else.
pub fn admits(caller: Caller<'_>, recipient: &str, products: &[&str]) -> Result<(), NotYours> {
    let fold = |s: &str| s.trim().to_ascii_lowercase();
    let refused = |said: String| Err(NotYours { recipient: recipient.trim().to_owned(), caller: said });
    match caller {
        Caller::Principal(who) if !recipient.trim().is_empty() && fold(who) == fold(recipient) => Ok(()),
        Caller::Principal(who) => refused(format!("the principal {who}")),
        Caller::Product(id) if products.contains(&id) => Ok(()),
        Caller::Product(id) => refused(format!("the product {id}, not on this face's callers")),
        Caller::Unknown => refused("a caller with no principal".to_owned()),
    }
}

/// One page the host read from the product's log, at the offset the scan asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page<E> {
    /// The events read, each at its offset, and the offset the next read starts at. No events and `next` where the
    /// read started: the log's head.
    Read {
        /// The events, each with its offset.
        events: Vec<(u64, E)>,
        /// Where the next read starts.
        next: u64,
    },
    /// The offset asked is at or below the log's compacted floor: every offset through `floor` is gone.
    Compacted {
        /// The last offset compacted.
        floor: u64,
    },
}

/// What `Cursor/Next` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer<E> {
    /// The recipient's items past `after`, at most `max`, and how far the scan read: `scanned_to` at the head with no
    /// items is caught up; short of it, read on from `scanned_to`.
    Items {
        /// Each item at its position.
        items: Vec<(Pos, E)>,
        /// The last position the scan read (`after` when it read none).
        scanned_to: Pos,
    },
    /// The recipient's next position is below the log's retained start: `from` through `to`, inclusive, were trimmed
    /// before they were read.
    Trimmed {
        /// The first position trimmed past `after`.
        from: Pos,
        /// The last position trimmed.
        to: Pos,
    },
}

/// What the host does next for a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step<E> {
    /// Read a page from this offset, at most `max` events, within what the deadline leaves.
    Read {
        /// The offset to read from.
        from: u64,
    },
    /// Answer the call with this.
    Answer(Answer<E>),
}

/// One `Cursor/Next`'s scan: page by page from `after`, filtered by the product's `Whose`, until it holds `max`
/// items, reaches the head, or its deadline passes.
#[derive(Clone, Debug)]
pub struct Scan<E> {
    after: Pos,
    max: usize,
    deadline: Mono,
    items: Vec<(Pos, E)>,
    scanned_to: Pos,
    answered: bool,
}

impl<E> Scan<E> {
    /// A scan for items past `after`, at most `max` (at least one), bounded by `deadline`, over a log whose first
    /// retained offset is `start`. Answers `Trimmed` at once when the recipient's next position is below it.
    #[must_use]
    pub fn new(after: Pos, max: usize, deadline: Mono, start: u64) -> (Scan<E>, Step<E>) {
        let mut scan = Scan { after, max: max.max(1), deadline, items: Vec::new(), scanned_to: after, answered: false };
        // The first retained position is `start + 1`; the recipient's next is `after + 1`.
        let step = if after < start {
            scan.answer(Answer::Trimmed { from: after + 1, to: start })
        } else {
            // Offset `after` holds position `after + 1`, the recipient's next.
            Step::Read { from: after }
        };
        (scan, step)
    }

    /// The deadline the host holds this scan to: past it, [`Scan::bound`].
    #[must_use]
    pub fn deadline(&self) -> Mono {
        self.deadline
    }

    /// The most events the next page should hold.
    #[must_use]
    pub fn want(&self) -> usize {
        self.max.saturating_sub(self.items.len()).max(1)
    }

    /// Take the page read, at `now`, keeping what `whose` says is this recipient's.
    pub fn page(&mut self, page: Page<E>, now: Mono, mut whose: impl FnMut(&E) -> bool) -> Step<E> {
        if self.answered {
            return self.answered_already();
        }
        match page {
            Page::Compacted { floor } => {
                let to = position_of(floor);
                if self.items.is_empty() && self.scanned_to < to {
                    // Nothing read yet: the gap is the recipient's to be told of, by name.
                    return self.answer(Answer::Trimmed { from: self.scanned_to + 1, to });
                }
                // A trim raced the scan: answer what was found; the next call is told of the gap.
                self.items_answer()
            }
            Page::Read { events, next } => {
                // The head: nothing read and the read's next where it started. A page of deleted entries only is empty
                // and moves `next`, so it reads on.
                let at_head = events.is_empty() && next <= self.scanned_to;
                for (offset, event) in events {
                    let at = position_of(offset);
                    if at <= self.scanned_to {
                        continue;
                    }
                    self.scanned_to = at;
                    if whose(&event) {
                        self.items.push((at, event));
                        if self.items.len() >= self.max {
                            return self.items_answer();
                        }
                    }
                }
                // Offsets below `next` were read (a deleted one leaves a hole): through position `next`.
                self.scanned_to = self.scanned_to.max(next);
                if at_head || now >= self.deadline {
                    return self.items_answer();
                }
                Step::Read { from: self.scanned_to }
            }
        }
    }

    /// The deadline passed before the next page: answer what was found, with how far the scan read.
    pub fn bound(&mut self) -> Step<E> {
        if self.answered {
            return self.answered_already();
        }
        self.items_answer()
    }

    fn items_answer(&mut self) -> Step<E> {
        let items = std::mem::take(&mut self.items);
        self.answer(Answer::Items { items, scanned_to: self.scanned_to })
    }

    fn answer(&mut self, answer: Answer<E>) -> Step<E> {
        self.answered = true;
        Step::Answer(answer)
    }

    fn answered_already(&self) -> Step<E> {
        Step::Answer(Answer::Items { items: Vec::new(), scanned_to: self.scanned_to })
    }

    /// The scan's invariants: it never reads behind `after`, and never holds more than `max`.
    ///
    /// # Errors
    /// The first broken, said.
    pub fn invariants(&self) -> Result<(), String> {
        if self.scanned_to < self.after {
            return Err(format!("scanned to {} behind after {}", self.scanned_to, self.after));
        }
        if self.items.len() > self.max {
            return Err(format!("{} items held past max {}", self.items.len(), self.max));
        }
        if let Some((at, _)) = self.items.iter().find(|(at, _)| *at <= self.after || *at > self.scanned_to) {
            return Err(format!("an item at {at} outside ({}, {}]", self.after, self.scanned_to));
        }
        Ok(())
    }
}
