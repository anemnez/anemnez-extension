//! The cursor face kernel's rows (CONTRACT.md §2a): each over hydramq-cursor's own `Marks`, so the face is proven on
//! the one mark kernel it maps onto, never a copy.

use std::time::Duration;

use anemnez_clock::{ManualClock, Mono};
use hydramq_cursor::{Change, Marks};

use crate::cursor_face::{
    Answer, Caller, Commit, CommitRefused, Mark, Marked, NotYours, Page, Scan, Step, admits, commit, marked,
};

/// `marks` after the face's commit of `mark` through `through` on a log ending at `end`: the change it decided,
/// applied as the store would.
fn committed(marks: &mut Marks, mark: Mark, through: u64, end: u64) -> Result<Marked, CommitRefused> {
    match commit(marks, mark, through, end)? {
        Commit::Done(now) => Ok(now),
        Commit::Write(change) => {
            marks.apply(&change);
            Ok(marked(marks))
        }
    }
}

/// A start and the deadline a call's 1.5 s bound sets from it: the kernel is handed `now`, so rows keep their own.
fn soon() -> (Mono, Mono) {
    let start = ManualClock::new().now();
    (start, start + Duration::from_millis(1500))
}

/// A page of `(offset, recipient)` events from `from`, `n` of them, as the host would read them.
fn page(log: &[&'static str], from: u64, n: usize) -> Page<&'static str> {
    let events: Vec<(u64, &'static str)> =
        log.iter().enumerate().skip(usize::try_from(from).unwrap()).take(n).map(|(o, e)| (o as u64, *e)).collect();
    let next = events.last().map_or(from, |(o, _)| o + 1);
    Page::Read { events, next }
}

/// Drive a scan to its answer over `log`, reading `per_page` at a time and moving the clock `cost` per page.
fn scan_all(
    log: &[&'static str],
    after: u64,
    max: usize,
    start: u64,
    per_page: usize,
    cost: Duration,
    recipient: &'static str,
) -> Answer<&'static str> {
    let (mut now, deadline) = soon();
    let (mut scan, mut step) = Scan::new(after, max, deadline, start);
    loop {
        scan.invariants().expect("the scan's invariants hold");
        match step {
            Step::Answer(answer) => return answer,
            Step::Read { from } => {
                if now >= scan.deadline() {
                    step = scan.bound();
                    continue;
                }
                let read = page(log, from, per_page.min(scan.want()));
                now = now + cost;
                step = scan.page(read, now, |e| *e == recipient);
            }
        }
    }
}

/// A commit only rises, and a repeat is done: an `acked` at or below the mark answers the mark as it stands and
/// writes nothing, so a client reads "ours or further" (rule 5).
#[test]
fn a_commit_only_rises_and_a_repeat_is_done() {
    let mut marks = Marks::default();
    assert_eq!(committed(&mut marks, Mark::Acked, 5, 10), Ok(Marked { acked: 5, handed: 5 }));
    assert_eq!(commit(&marks, Mark::Acked, 5, 10), Ok(Commit::Done(Marked { acked: 5, handed: 5 })), "a repeat");
    assert_eq!(commit(&marks, Mark::Acked, 3, 10), Ok(Commit::Done(Marked { acked: 5, handed: 5 })), "a lower one");
    assert_eq!(committed(&mut marks, Mark::Acked, 7, 10), Ok(Marked { acked: 7, handed: 7 }));
    assert_eq!(commit(&marks, Mark::Handed, 6, 10), Ok(Commit::Done(Marked { acked: 7, handed: 7 })));
}

/// The face's positions are hydramq-cursor's exactly: `acked = p` is `next = p`, every item through `p` acked, and a
/// fresh recipient's marks are 0.
#[test]
fn acked_through_a_position_is_the_cursors_next_exactly() {
    let mut marks = Marks::default();
    assert_eq!(marked(&marks), Marked { acked: 0, handed: 0 }, "a fresh recipient");
    assert_eq!(commit(&marks, Mark::Acked, 4, 9), Ok(Commit::Write(Change::Ack { through: 4 })));
    committed(&mut marks, Mark::Acked, 4, 9).expect("commits");
    assert_eq!(marks.next(), 4);
    assert!(marks.is_acked(3), "offset 3, position 4, is acked");
    assert!(!marks.is_acked(4), "offset 4, position 5, is not");
}

/// `commit-past-end` names the end: a commit past the log's last position is refused, whatever the mark, for both
/// marks, and its words carry the name and the end.
#[test]
fn a_commit_past_the_end_is_refused_naming_the_end() {
    let marks = Marks::default();
    for mark in [Mark::Acked, Mark::Handed] {
        assert_eq!(commit(&marks, mark, 11, 10), Err(CommitRefused::PastEnd { end: 10 }));
    }
    assert_eq!(commit(&marks, Mark::Acked, 10, 10), Ok(Commit::Write(Change::Ack { through: 10 })), "the end is fine");
    let said = CommitRefused::PastEnd { end: 10 }.to_string();
    assert!(said.starts_with("commit-past-end: "), "{said}");
    assert!(said.contains("10"), "{said}");
    assert_eq!(commit(&marks, Mark::Acked, 1, 0), Err(CommitRefused::PastEnd { end: 0 }), "an empty log");
}

/// An `acked` past `handed` raises `handed` with it, in the one write, never refused: `handed` is advisory, and a
/// client's batched `handed` may lag its ack.
#[test]
fn an_acked_past_handed_raises_handed() {
    let mut marks = Marks::default();
    assert_eq!(committed(&mut marks, Mark::Handed, 3, 10), Ok(Marked { acked: 0, handed: 3 }));
    assert_eq!(commit(&marks, Mark::Acked, 6, 10), Ok(Commit::Write(Change::Ack { through: 6 })), "one write");
    assert_eq!(committed(&mut marks, Mark::Acked, 6, 10), Ok(Marked { acked: 6, handed: 6 }));
    // A handed past acked is kept past it.
    assert_eq!(committed(&mut marks, Mark::Handed, 9, 10), Ok(Marked { acked: 6, handed: 9 }));
    assert_eq!(committed(&mut marks, Mark::Acked, 8, 10), Ok(Marked { acked: 8, handed: 9 }));
}

/// Only the recipient, or a product the face lists, reads or moves its marks; anyone else is `cursor-not-yours`,
/// named with who asked.
#[test]
fn an_unlisted_caller_is_refused_cursor_not_yours() {
    let products = ["ai.anemnez.agorad"];
    assert_eq!(admits(Caller::Principal("H"), "h", &products), Ok(()), "names fold as the store keys them");
    assert_eq!(admits(Caller::Product("ai.anemnez.agorad"), "H", &products), Ok(()));
    for (caller, said) in [
        (Caller::Principal("D"), "the principal D"),
        (Caller::Product("ai.anemnez.stoad"), "the product ai.anemnez.stoad, not on this face's callers"),
        (Caller::Unknown, "a caller with no principal"),
    ] {
        let refused = admits(caller, "H", &products);
        assert_eq!(refused, Err(NotYours { recipient: "H".to_owned(), caller: said.to_owned() }));
        assert!(refused.unwrap_err().to_string().starts_with("cursor-not-yours: "));
    }
    assert!(admits(Caller::Principal(""), " ", &products).is_err(), "an empty recipient is no one's");
}

/// `Next` answers the recipient's items past `after`, at most `max`, each at its one-based position, with how far it
/// read; `max` cuts the page.
#[test]
fn next_answers_the_recipients_items_past_after_at_most_max() {
    let log = ["H", "D", "H", "H", "M", "H"];
    let all = scan_all(&log, 0, 10, 0, 2, Duration::ZERO, "H");
    assert_eq!(all, Answer::Items { items: vec![(1, "H"), (3, "H"), (4, "H"), (6, "H")], scanned_to: 6 });
    let cut = scan_all(&log, 1, 2, 0, 10, Duration::ZERO, "H");
    assert_eq!(cut, Answer::Items { items: vec![(3, "H"), (4, "H")], scanned_to: 4 }, "max cuts the page");
    let rest = scan_all(&log, 4, 2, 0, 10, Duration::ZERO, "H");
    assert_eq!(rest, Answer::Items { items: vec![(6, "H")], scanned_to: 6 }, "from where it left off");
}

/// A sparse recipient over a long log (E m/43868): the scan is cut by the call's own bound, answers what it found
/// with `scanned_to` short of the head, and is never unanswered; read on from there, it reaches its item.
#[test]
fn a_sparse_recipient_over_a_long_log_is_cut_at_the_bound_with_scanned_to() {
    let mut log = vec!["D"; 400];
    log.push("H");
    let cost = Duration::from_millis(100);
    let first = scan_all(&log, 0, 10, 0, 10, cost, "H");
    let Answer::Items { items, scanned_to } = first else { panic!("items, never trimmed: {first:?}") };
    assert!(items.is_empty(), "nothing of H's in what it could read");
    assert_eq!(scanned_to, 150, "fifteen pages inside 1.5 s, at 100 ms each");
    let mut after = scanned_to;
    let found = loop {
        match scan_all(&log, after, 10, 0, 10, cost, "H") {
            Answer::Items { items, scanned_to } if items.is_empty() => {
                assert!(scanned_to > after, "each call reads on");
                after = scanned_to;
            }
            other => break other,
        }
    };
    assert_eq!(found, Answer::Items { items: vec![(401, "H")], scanned_to: 401 });
}

/// An empty page short of the head says `scanned_to` and is not caught up: only `scanned_to` at the head is.
#[test]
fn an_empty_page_short_of_the_head_is_not_caught_up() {
    let log = ["D"; 30];
    let (now, deadline) = soon();
    let (mut scan, step) = Scan::<&str>::new(0, 5, deadline, 0);
    assert_eq!(step, Step::Read { from: 0 });
    let cut = scan.page(page(&log, 0, 10), now + Duration::from_secs(2), |e| *e == "H");
    assert_eq!(cut, Step::Answer(Answer::Items { items: vec![], scanned_to: 10 }), "short of the head at 30");
    let at_head = scan_all(&log, 30, 5, 0, 10, Duration::ZERO, "H");
    assert_eq!(at_head, Answer::Items { items: vec![], scanned_to: 30 }, "at the head: caught up");
}

/// A page of deleted entries only moves the read on, never reads as the head.
#[test]
fn a_page_of_holes_reads_on() {
    let (now, deadline) = soon();
    let (mut scan, _) = Scan::<&str>::new(0, 5, deadline, 0);
    let step = scan.page(Page::Read { events: vec![], next: 4 }, now, |_| true);
    assert_eq!(step, Step::Read { from: 4 }, "offsets 0 to 3 were holes");
    let step = scan.page(Page::Read { events: vec![(4, "H")], next: 5 }, now, |_| true);
    assert_eq!(step, Step::Read { from: 5 });
    let step = scan.page(Page::Read { events: vec![], next: 5 }, now, |_| true);
    assert_eq!(step, Step::Answer(Answer::Items { items: vec![(5, "H")], scanned_to: 5 }));
}

/// `Next` below the retained start is `Trimmed { from, to }`, inclusive, by name: from the log's start, and from a
/// read that meets the compacted floor. A client that acks through `to` is handed the first retained item next.
#[test]
fn next_below_the_retained_start_is_trimmed() {
    // Offsets 0..12 retained from 12 on: positions 1 to 12 are gone.
    let log = ["H"; 20];
    let trimmed = scan_all(&log, 4, 5, 12, 10, Duration::ZERO, "H");
    assert_eq!(trimmed, Answer::Trimmed { from: 5, to: 12 }, "eight items, 5 to 12");
    let mut marks = Marks::default();
    committed(&mut marks, Mark::Acked, 12, 20).expect("acks through to");
    let next = scan_all(&log, marked(&marks).acked, 1, 12, 10, Duration::ZERO, "H");
    assert_eq!(next, Answer::Items { items: vec![(13, "H")], scanned_to: 13 }, "the first retained");
    let at_start = scan_all(&log, 12, 1, 12, 10, Duration::ZERO, "H");
    assert!(matches!(at_start, Answer::Items { .. }), "at the start nothing is trimmed: {at_start:?}");

    let (now, deadline) = soon();
    let (mut scan, step) = Scan::<&str>::new(2, 5, deadline, 0);
    assert_eq!(step, Step::Read { from: 2 });
    let step = scan.page(Page::Compacted { floor: 7 }, now, |_| true);
    assert_eq!(step, Step::Answer(Answer::Trimmed { from: 3, to: 8 }), "offsets through 7: positions 3 to 8");
}

/// A trim that races a scan with items in hand answers the items; the next call is told of the gap.
#[test]
fn a_trim_racing_a_scan_answers_what_it_found() {
    let (now, deadline) = soon();
    let (mut scan, _) = Scan::new(0, 5, deadline, 0);
    let step = scan.page(Page::Read { events: vec![(0, "H")], next: 1 }, now, |_| true);
    assert_eq!(step, Step::Read { from: 1 });
    let step = scan.page(Page::Compacted { floor: 6 }, now, |_| true);
    assert_eq!(step, Step::Answer(Answer::Items { items: vec![(1, "H")], scanned_to: 1 }));
}

/// Two recipients' marks never touch: each is its own `Marks`, and one's commit decides nothing of the other's.
#[test]
fn two_recipients_marks_never_touch() {
    let (mut h, mut d) = (Marks::default(), Marks::default());
    committed(&mut h, Mark::Acked, 6, 10).expect("H commits");
    committed(&mut d, Mark::Handed, 2, 10).expect("D hands");
    assert_eq!((marked(&h), marked(&d)), (Marked { acked: 6, handed: 6 }, Marked { acked: 0, handed: 2 }));
    let log = ["H", "D", "H", "D"];
    let both = (scan_all(&log, 0, 5, 0, 4, Duration::ZERO, "H"), scan_all(&log, 0, 5, 0, 4, Duration::ZERO, "D"));
    assert_eq!(
        both,
        (
            Answer::Items { items: vec![(1, "H"), (3, "H")], scanned_to: 4 },
            Answer::Items { items: vec![(2, "D"), (4, "D")], scanned_to: 4 },
        )
    );
}

/// Explored: every commit sequence of up to four steps over small positions keeps the face's marks rising and
/// `handed` at or past `acked`, and every answered mark is what the kernel's marks say.
#[test]
fn every_short_commit_sequence_keeps_the_marks_rising() {
    let steps: Vec<(Mark, u64)> =
        [Mark::Acked, Mark::Handed].into_iter().flat_map(|m| (0..=5).map(move |p| (m, p))).collect();
    let mut seqs: Vec<Vec<(Mark, u64)>> = vec![vec![]];
    for _ in 0..4 {
        seqs = seqs.iter().flat_map(|s| steps.iter().map(move |st| [s.clone(), vec![*st]].concat())).collect();
    }
    for seq in &seqs {
        let mut marks = Marks::default();
        let mut last = Marked::default();
        for (mark, through) in seq {
            let now = committed(&mut marks, *mark, *through, 4);
            match now {
                Err(CommitRefused::PastEnd { end }) => assert!(*through > end, "{seq:?}"),
                Ok(now) => {
                    assert!(now.acked >= last.acked && now.handed >= last.handed, "rose: {seq:?}");
                    assert!(now.handed >= now.acked, "handed at or past acked: {seq:?}");
                    assert_eq!(now, marked(&marks), "the answer is the marks: {seq:?}");
                    let moved = match mark {
                        Mark::Acked => now.acked,
                        Mark::Handed => now.handed,
                    };
                    assert!(moved >= *through, "ours or further: {seq:?}");
                    last = now;
                }
            }
            marks.invariants().expect("hydramq-cursor's invariants hold");
        }
    }
}
