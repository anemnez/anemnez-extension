//! The Watch driver's rows: its invariant over every short input sequence, and each way a stream ends.

use crate::watch::{Driver, End, Input, Output, State};

fn inputs() -> Vec<Input> {
    vec![
        Input::Frame(vec![1]),
        Input::ProducerEnded,
        Input::Drain,
        Input::Expired,
        Input::ConsumerGone,
        Input::Revoked,
        Input::Lapsed,
    ]
}

/// Every way to end stops the wire watch first, exactly once, and nothing is sent or stopped after the end:
/// explored over every sequence of up to five inputs.
#[test]
fn every_exit_stops_the_wire_watch_first_and_once() {
    fn explore(driver: &Driver, outputs: &[Output], depth: usize) {
        let stops = outputs.iter().filter(|o| **o == Output::Stop).count();
        assert!(stops <= 1, "stopped twice: {outputs:?}");
        if let State::Ended(_) = driver.state() {
            let end = outputs.iter().position(|o| matches!(o, Output::End(_))).expect("an end was said");
            let stop = outputs.iter().position(|o| *o == Output::Stop).expect("an end without a stop");
            assert!(stop < end, "the stop comes before the end: {outputs:?}");
            assert_eq!(end, outputs.len() - 1, "nothing after the end: {outputs:?}");
        }
        if depth == 0 {
            return;
        }
        for input in inputs() {
            let mut next = clone(driver);
            let mut seen = outputs.to_vec();
            seen.extend(next.step(input));
            explore(&next, &seen, depth - 1);
        }
    }
    explore(&Driver::new(), &[], 5);
}

fn clone(d: &Driver) -> Driver {
    // The driver is two plain fields; rebuild it by replaying to the same state.
    let mut c = Driver::new();
    match d.state() {
        State::Streaming => {}
        State::Draining => {
            c.step(Input::Drain);
        }
        State::Ended(End::Producer) => {
            c.step(Input::ProducerEnded);
        }
        State::Ended(End::Draining) => {
            c.step(Input::Drain);
            c.step(Input::ProducerEnded);
        }
        State::Ended(End::Expired) => {
            c.step(Input::Expired);
        }
        State::Ended(End::ConsumerGone) => {
            c.step(Input::ConsumerGone);
        }
        State::Ended(End::Unauthorized) => {
            c.step(Input::Revoked);
        }
    }
    assert_eq!((c.state(), c.stopped()), (d.state(), d.stopped()));
    c
}

/// A drain mid-stream stops the watch at once, still sends what it had read, and ends `draining` once the
/// thread has handed over its last frame.
#[test]
fn a_drain_sends_what_was_read_then_ends_draining() {
    let mut d = Driver::new();
    assert_eq!(d.step(Input::Frame(vec![1])), vec![Output::Send(vec![1])]);
    assert_eq!(d.step(Input::Drain), vec![Output::Stop]);
    assert_eq!(d.step(Input::Frame(vec![2])), vec![Output::Send(vec![2])], "read before the ask, so sent");
    assert_eq!(d.step(Input::Drain), Vec::new(), "a second ask changes nothing");
    assert_eq!(d.step(Input::ProducerEnded), vec![Output::End(End::Draining)]);
    assert_eq!(End::Draining.refusal(), Some("draining"));
}

/// The token's expiry ends the stream `expired`, draining or not, and the consumer going away ends it quietly.
#[test]
fn expiry_ends_expired_and_a_gone_consumer_ends_quietly() {
    let mut d = Driver::new();
    assert_eq!(d.step(Input::Expired), vec![Output::Stop, Output::End(End::Expired)]);
    assert_eq!(End::Expired.refusal(), Some("expired"));
    let mut draining = Driver::new();
    draining.step(Input::Drain);
    assert_eq!(draining.step(Input::Expired), vec![Output::End(End::Expired)], "already stopped, so no second stop");
    let mut gone = Driver::new();
    assert_eq!(gone.step(Input::ConsumerGone), vec![Output::Stop, Output::End(End::ConsumerGone)]);
    assert_eq!(End::ConsumerGone.refusal(), None);
    assert_eq!(gone.step(Input::Frame(vec![9])), Vec::new(), "nothing is sent after the end");
}

/// a-watch-ends-when-its-authorization-does: the product's revoke, or the end its authorization said at admit,
/// ends the stream `unauthorized`, stopping the wire watch first; what was sent stays sent and nothing more is,
/// draining or not.
#[test]
fn a_revoke_or_a_lapse_ends_unauthorized_after_what_was_sent() {
    let mut d = Driver::new();
    assert_eq!(d.step(Input::Frame(vec![1])), vec![Output::Send(vec![1])]);
    assert_eq!(d.step(Input::Revoked), vec![Output::Stop, Output::End(End::Unauthorized)]);
    assert_eq!(d.step(Input::Frame(vec![2])), Vec::new(), "nothing is sent after the end");
    assert_eq!(End::Unauthorized.refusal(), Some("unauthorized"));
    let mut lapsed = Driver::new();
    assert_eq!(lapsed.step(Input::Lapsed), vec![Output::Stop, Output::End(End::Unauthorized)]);
    let mut draining = Driver::new();
    draining.step(Input::Drain);
    assert_eq!(
        draining.step(Input::Lapsed),
        vec![Output::End(End::Unauthorized)],
        "already stopped, so no second stop"
    );
}
