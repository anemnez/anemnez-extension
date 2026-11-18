//! The key cache in its host, on the deterministic simulation (E m/5 direct, D m/17459): concurrent tokens from one
//! issuer, a keys source whose read takes a seeded time on the run's clock, and the flight's deadline. Below it every
//! token is admitted on one read; at or past it every waiter is `lookup-unanswered` (keys), by name, at the deadline.
//! Each seed runs twice and gives one trace. Without `--cfg tokio_unstable` the runtime's draw isn't seeded, so the row
//! refuses by name rather than prove a replay it can't make.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::principal::{Lookup, Refusal};

use crate::key_cache::FETCH_WITHIN;
use crate::principal::{Keys, MemoryNonces, Verifier};
use crate::principal_tests::{AUDIENCE, ISSUER, mint_on, published};

const METHOD: &str = "/anemnez.extension.v1.Events/Events";
const SEEDS: u64 = 24;

/// One issuer's record, read in a seeded time on the run's clock, counting reads.
struct Slow {
    clock: Clock,
    takes: Duration,
    reads: AtomicU32,
}

#[tonic::async_trait]
impl Keys for Slow {
    async fn record(&self, _issuer: &str) -> Result<crate::KeyRecord, Refusal> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.clock.sleep(self.takes).await;
        Ok(crate::KeyRecord { issuer: ISSUER.into(), keys: vec![published(&[7; 32])] })
    }
}

/// One seed's run: what each token was answered, in order, how many reads, and when the last answer came.
fn trace(seed: u64) -> (Vec<String>, u32, Duration, Duration) {
    anemnez_sim::run(seed, |mut world| async move {
        let sim = anemnez_sim::SimClock::new();
        let clock = sim.clock();
        let window = u64::try_from(FETCH_WITHIN.as_millis()).unwrap_or(400);
        let takes = Duration::from_millis(world.draws.next_u64() % (2 * window));
        let tokens = 2 + world.draws.next_u64() % 9;
        let keys = Arc::new(Slow { clock: clock.clone(), takes, reads: AtomicU32::new(0) });
        let verifier = Verifier::new(
            AUDIENCE,
            clock.clone(),
            Arc::clone(&keys) as Arc<dyn Keys>,
            Arc::new(MemoryNonces::new(clock.clone())),
        );
        let started = clock.now_mono();
        let asks: Vec<_> = (0..tokens)
            .map(|n| {
                let verifier = verifier.clone();
                let token = mint_on(&clock, METHOD, [1; 32], 60, u8::try_from(n + 1).unwrap_or(1));
                tokio::spawn(async move { verifier.verify(&token, METHOD, &[1; 32]).await.map(|_| ()) })
            })
            .collect();
        let mut answers = Vec::new();
        for ask in asks {
            answers.push(format!("{:?}", ask.await.expect("joins")));
        }
        (answers, keys.reads.load(Ordering::SeqCst), clock.now_mono().saturating_since(started), takes)
    })
}

#[test]
fn tokens_on_one_flight_are_admitted_on_one_read_or_unclear_at_its_deadline_and_each_seed_replays() {
    // Refused by name on a build whose runtime draw isn't seeded: it couldn't prove a replay.
    #[cfg(not(tokio_unstable))]
    panic!("{}", anemnez_sim::run::unseeded(0));
    let unclear = format!("{:?}", Err::<(), _>(Refusal::LookupUnanswered(Lookup::Keys)));
    for seed in 0..SEEDS {
        let (answers, reads, took, takes) = trace(seed);
        assert_eq!(reads, 1, "seed {seed}: the tokens joined one flight");
        if takes < FETCH_WITHIN {
            assert!(answers.iter().all(|a| a == "Ok(())"), "seed {seed}: read in {takes:?}, all admitted: {answers:?}");
        } else {
            assert!(answers.iter().all(|a| *a == unclear), "seed {seed}: read in {takes:?}, all Unclear: {answers:?}");
            assert_eq!(took, FETCH_WITHIN, "seed {seed}: answered at the flight's deadline, not after");
        }
        assert_eq!(trace(seed), (answers, reads, took, takes), "seed {seed} replays");
    }
}
