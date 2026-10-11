//! Hash every tick of seeded Trials; persist and reconstruct their commands.
use salt_line_sim::{
    ballistics::Shot,
    replay::Hasher,
    rng::Pcg32,
    trial::{FireCommand, Replay, Trial, ROUND_TICKS, RULES_VERSION},
};
fn main() {
    let mut digest = Hasher::new();
    let mut hits = 0;
    for seed in 0..200 {
        let mut input = Pcg32::new(seed ^ 0xD20E);
        let commands: Vec<_> = (0..ROUND_TICKS)
            .step_by(90)
            .map(|tick| FireCommand {
                tick,
                shot: Shot::new(
                    input.range_u32(200, 1500) as i32,
                    input.range_u32(300, 1000) as i32,
                ),
            })
            .collect();
        let replay = Replay {
            seed,
            rules_version: RULES_VERSION,
            commands,
        };
        let decoded = Replay::decode(&replay.encode().expect("encode")).expect("decode");
        let mut live = Trial::new(seed);
        let mut recorded = decoded.commands.iter().peekable();
        for tick in 0..ROUND_TICKS {
            while recorded.peek().is_some_and(|c| c.tick == tick) {
                live.fire(recorded.next().unwrap().shot);
            }
            live.step();
            digest.write_u64(live.state_hash());
            live.take_events();
        }
        assert_eq!(
            live.state_hash(),
            decoded.play().expect("replay").state_hash(),
            "seed {seed}"
        );
        hits += live.score() / 25;
    }
    println!("Drone Trial: 200 persisted replays, {hits} hits, zero divergences");
    assert!(hits > 0, "digest workload must exercise hits");
    println!("digest: {:#018x}", digest.finish());
}
