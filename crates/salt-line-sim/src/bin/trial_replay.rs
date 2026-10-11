//! Verify a recorded local Drone Trial without running Godot.
use salt_line_sim::trial::Replay;
use std::io::Read;
fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or("usage: trial_replay <replay-file>")?;
    if args.next().is_some() {
        return Err("usage: trial_replay <replay-file>".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(1_048_577)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    let replay = Replay::decode(&text).map_err(|e| format!("invalid replay: {e:?}"))?;
    let trial = replay.play().map_err(|e| format!("replay failed: {e:?}"))?;
    println!(
        "seed {}: score {}, shells {}, tick {}, state {:#018x}",
        replay.seed,
        trial.score(),
        trial.ammo(),
        trial.tick(),
        trial.state_hash()
    );
    Ok(())
}
