//! A run as text: version, seed, width and inputs. Replaying it rebuilds the
//! run exactly (the determinism contract), so it doubles as a save and a
//! shareable ghost.
//!
//! Format: `SHIFT<version>:<seed hex>:<width>:<inputs>`, inputs separated by
//! `.`: `s` shift, `f<angle>,<power>` fire, `p<n>` pick, `k` skip.

use crate::expedition::{Expedition, Input};
use crate::SHIFT_LOG_VERSION;

/// Why a log could not be replayed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LogError {
    /// Not a shift log, or a field failed to parse.
    Malformed,
    /// Written by another version of the rules.
    Version(u32),
    /// Input `n` was not valid when it was reached.
    Rejected(usize),
}

/// Encodes a run's log.
pub fn encode(run: &Expedition) -> String {
    let inputs: Vec<String> = run
        .log()
        .iter()
        .map(|i| match *i {
            Input::Shift => "s".to_string(),
            Input::Fire {
                angle_ticks,
                power_ticks,
            } => format!("f{angle_ticks},{power_ticks}"),
            Input::Pick(n) => format!("p{n}"),
            Input::Skip => "k".to_string(),
        })
        .collect();
    format!(
        "SHIFT{SHIFT_LOG_VERSION}:{:x}:{}:{}",
        run.seed(),
        run.width(),
        inputs.join(".")
    )
}

/// Rebuilds a run from its log.
pub fn replay(text: &str) -> Result<Expedition, LogError> {
    let rest = text
        .trim()
        .strip_prefix("SHIFT")
        .ok_or(LogError::Malformed)?;
    let mut parts = rest.splitn(4, ':');
    let mut field = || parts.next().ok_or(LogError::Malformed);
    let version: u32 = field()?.parse().map_err(|_| LogError::Malformed)?;
    if version != SHIFT_LOG_VERSION {
        return Err(LogError::Version(version));
    }
    let seed = u64::from_str_radix(field()?, 16).map_err(|_| LogError::Malformed)?;
    let width: usize = field()?.parse().map_err(|_| LogError::Malformed)?;
    let inputs = field()?;
    let mut run = Expedition::new(seed, width);
    for (n, tok) in inputs.split('.').filter(|t| !t.is_empty()).enumerate() {
        let input = parse_input(tok).ok_or(LogError::Malformed)?;
        run.apply(input).ok_or(LogError::Rejected(n))?;
    }
    Ok(run)
}

fn parse_input(tok: &str) -> Option<Input> {
    let (head, body) = tok.split_at(1);
    match head {
        "s" if body.is_empty() => Some(Input::Shift),
        "k" if body.is_empty() => Some(Input::Skip),
        "p" => body.parse().ok().map(Input::Pick),
        "f" => {
            let (a, p) = body.split_once(',')?;
            Some(Input::Fire {
                angle_ticks: a.parse().ok()?,
                power_ticks: p.parse().ok()?,
            })
        }
        _ => None,
    }
}
