//! Target cosmetics only. Dates and preferences never reach the simulation.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Seasonal {
    #[default]
    Auto,
    Off,
    Halloween,
    Thanksgiving,
    Christmas,
}

impl Seasonal {
    pub fn token(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Halloween => "halloween",
            Self::Thanksgiving => "thanksgiving",
            Self::Christmas => "christmas",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        [
            Self::Auto,
            Self::Off,
            Self::Halloween,
            Self::Thanksgiving,
            Self::Christmas,
        ]
        .into_iter()
        .find(|s| s.token() == token)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Off => "OFF",
            Self::Halloween => "HALLOWEEN",
            Self::Thanksgiving => "THANKSGIVING",
            Self::Christmas => "CHRISTMAS",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Off,
            Self::Off => Self::Halloween,
            Self::Halloween => Self::Thanksgiving,
            Self::Thanksgiving => Self::Christmas,
            Self::Christmas => Self::Auto,
        }
    }

    /// UTC days since Unix epoch, the same presentation clock as daily runs.
    pub fn resolve(self, day: u64) -> Self {
        if self != Self::Auto {
            return self;
        }
        // Gregorian civil-from-days: month within a 400-year era, shifted to
        // start in March. No engine, timezone library or simulation state.
        let doe = day.saturating_add(719468) % 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        match month {
            10 => Self::Halloween,
            11 => Self::Thanksgiving,
            12 => Self::Christmas,
            _ => Self::Off,
        }
    }

    pub fn index(self) -> Option<usize> {
        match self {
            Self::Halloween => Some(0),
            Self::Thanksgiving => Some(1),
            Self::Christmas => Some(2),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_boundaries_and_manual_choices() {
        // 2026-09-30 through 2027-01-01; also leap-day 2024.
        for (day, expected) in [
            (20726, Seasonal::Off),
            (20727, Seasonal::Halloween),
            (20757, Seasonal::Halloween),
            (20758, Seasonal::Thanksgiving),
            (20787, Seasonal::Thanksgiving),
            (20788, Seasonal::Christmas),
            (20818, Seasonal::Christmas),
            (20819, Seasonal::Off),
            (19782, Seasonal::Off),
        ] {
            assert_eq!(Seasonal::Auto.resolve(day), expected, "day {day}");
            assert_eq!(Seasonal::Halloween.resolve(day), Seasonal::Halloween);
            assert_eq!(Seasonal::Off.resolve(day), Seasonal::Off);
        }
    }

    #[test]
    fn choice_cycle_and_tokens() {
        let mut season = Seasonal::Auto;
        for _ in 0..5 {
            assert_eq!(Seasonal::from_token(season.token()), Some(season));
            season = season.next();
        }
        assert_eq!(season, Seasonal::Auto);
        assert_eq!(Seasonal::from_token("unknown"), None);
    }
}
