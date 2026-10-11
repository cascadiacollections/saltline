//! Dependency-free, bounded text persistence for Drone Trial replays.
use crate::{
    ballistics::Shot,
    trial::{FireCommand, Replay, ReplayError, ROUND_TICKS, RULES_VERSION},
};
const MAX_BYTES: usize = 1_048_576;
const MAX_COMMANDS: usize = 16_384;

impl Replay {
    pub fn encode(&self) -> Result<String, ReplayError> {
        self.validate()?;
        let mut text = format!(
            "SALT_LINE_DRONE_TRIAL 1 {} {}\n",
            self.rules_version, self.seed
        );
        for c in &self.commands {
            text.push_str(&format!(
                "{} {} {}\n",
                c.tick, c.shot.angle_ticks, c.shot.power_ticks
            ));
        }
        Ok(text)
    }
    pub fn decode(text: &str) -> Result<Self, ReplayError> {
        if text.len() > MAX_BYTES {
            return Err(ReplayError::TooLarge);
        }
        let mut lines = text.lines();
        let header: Vec<_> = lines
            .next()
            .ok_or(ReplayError::InvalidFormat)?
            .split_whitespace()
            .collect();
        if header.len() != 4 || header[0] != "SALT_LINE_DRONE_TRIAL" || header[1] != "1" {
            return Err(ReplayError::InvalidFormat);
        }
        let rules_version = header[2].parse().map_err(|_| ReplayError::InvalidFormat)?;
        let seed = header[3].parse().map_err(|_| ReplayError::InvalidFormat)?;
        let mut commands = Vec::new();
        for line in lines {
            if commands.len() == MAX_COMMANDS {
                return Err(ReplayError::TooLarge);
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 3 {
                return Err(ReplayError::InvalidFormat);
            }
            commands.push(FireCommand {
                tick: fields[0].parse().map_err(|_| ReplayError::InvalidFormat)?,
                shot: Shot::new(
                    fields[1].parse().map_err(|_| ReplayError::InvalidFormat)?,
                    fields[2].parse().map_err(|_| ReplayError::InvalidFormat)?,
                ),
            });
        }
        let replay = Self {
            rules_version,
            seed,
            commands,
        };
        replay.validate()?;
        Ok(replay)
    }
    pub(crate) fn validate(&self) -> Result<(), ReplayError> {
        if self.rules_version != RULES_VERSION {
            return Err(ReplayError::UnsupportedRules);
        }
        if self.commands.len() > MAX_COMMANDS {
            return Err(ReplayError::TooLarge);
        }
        if self.commands.iter().any(|c| c.tick >= ROUND_TICKS)
            || self.commands.windows(2).any(|w| w[0].tick > w[1].tick)
        {
            return Err(ReplayError::InvalidTickOrder);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_preserves_duplicate_tick_order_and_outcomes() {
        let replay = Replay {
            seed: u64::MAX,
            rules_version: RULES_VERSION,
            commands: vec![
                FireCommand {
                    tick: 0,
                    shot: Shot::new(900, 650),
                },
                FireCommand {
                    tick: 0,
                    shot: Shot::new(800, 700),
                },
                FireCommand {
                    tick: 120,
                    shot: Shot::new(1100, 900),
                },
            ],
        };
        let decoded = Replay::decode(&replay.encode().unwrap()).unwrap();
        assert_eq!(decoded.commands, replay.commands);
        assert_eq!(
            decoded.play().unwrap().state_hash(),
            replay.play().unwrap().state_hash()
        );
    }
    #[test]
    fn rejects_corruption_versions_and_out_of_order_commands() {
        for text in [
            "",
            "SALT_LINE_DRONE_TRIAL 2 1 0",
            "SALT_LINE_ARTILLERY 1 1 0",
            "SALT_LINE_DRONE_TRIAL 1 1 0\n1 2",
            "SALT_LINE_DRONE_TRIAL 1 1 0\n2 0 0\n1 0 0",
            "SALT_LINE_DRONE_TRIAL 1 1 0\n3600 0 0",
        ] {
            assert!(Replay::decode(text).is_err(), "accepted {text}");
        }
        assert!(matches!(
            Replay::decode("SALT_LINE_DRONE_TRIAL 1 99 0"),
            Err(ReplayError::UnsupportedRules)
        ));
        assert!(matches!(
            Replay::decode(&"x".repeat(MAX_BYTES + 1)),
            Err(ReplayError::TooLarge)
        ));
    }
}
