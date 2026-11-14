//! Writes the synthesized cues (SHIFT whoosh, the two ambient loops) as WAV
//! files, for listening to them outside the game.
//!
//! `cargo run --release -p shift-game --example sounds -- <out dir>`

use shift_game::audio::{pcm, Cue, MIX_RATE};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "sounds".into());
    std::fs::create_dir_all(&dir).unwrap();
    for (cue, name) in [
        (Cue::Shift, "shift"),
        (Cue::AmbPresent, "ambience-present"),
        (Cue::AmbSiege, "ambience-1347"),
    ] {
        let data = pcm(cue);
        let mut w = Vec::with_capacity(44 + data.len());
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&MIX_RATE.to_le_bytes());
        w.extend_from_slice(&(MIX_RATE * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        let path = format!("{dir}/{name}.wav");
        std::fs::write(&path, w).unwrap();
        println!("{path}");
    }
}
