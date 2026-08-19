use chrono::{DateTime, TimeDelta, Utc};
use eyre::{Context, ContextCompat, Result, bail};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeRange {
    from: String,
    to: String,
}

impl TimeRange {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }

    pub(crate) fn from(&self) -> &str {
        &self.from
    }

    pub(crate) fn to(&self) -> &str {
        &self.to
    }

    pub(crate) fn resolve(&self, now: DateTime<Utc>) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
        let from = parse_time(self.from(), now)
            .wrap_err_with(|| format!("invalid --from value {:?}", self.from()))?;
        let to = parse_time(self.to(), now)
            .wrap_err_with(|| format!("invalid --to value {:?}", self.to()))?;
        if from > to {
            bail!("--from must not be later than --to");
        }
        Ok((from, to))
    }
}

impl Default for TimeRange {
    fn default() -> Self {
        Self::new("now-1h", "now")
    }
}

fn parse_time(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    if value == "now" {
        return Ok(now);
    }

    if let Some((sign, offset)) = value
        .strip_prefix("now-")
        .map(|offset| (-1, offset))
        .or_else(|| value.strip_prefix("now+").map(|offset| (1, offset)))
    {
        let (amount, unit) = offset.split_at(offset.len().saturating_sub(1));
        let amount = amount
            .parse::<i64>()
            .context("relative time must contain a positive whole number")?;
        if amount <= 0 {
            bail!("relative time must contain a positive whole number");
        }
        let seconds_per_unit = match unit {
            "s" => 1,
            "m" => 60,
            "h" => 60 * 60,
            "d" => 24 * 60 * 60,
            "w" => 7 * 24 * 60 * 60,
            _ => bail!("relative time unit must be one of s, m, h, d, or w"),
        };
        let seconds = amount
            .checked_mul(seconds_per_unit)
            .context("relative time is too large")?;
        let delta = TimeDelta::try_seconds(seconds).context("relative time is too large")?;
        return now
            .checked_add_signed(delta * sign)
            .context("relative time is out of range");
    }

    value
        .parse::<DateTime<Utc>>()
        .context("expected now, now-<duration>, now+<duration>, or an RFC 3339 timestamp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_absolute_times() {
        let now = "2026-08-19T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

        assert_eq!(
            TimeRange::new("now-6h", "now-30m").resolve(now).unwrap(),
            (
                "2026-08-19T06:00:00Z".parse().unwrap(),
                "2026-08-19T11:30:00Z".parse().unwrap(),
            )
        );
        assert_eq!(
            TimeRange::new("2026-08-18T12:00:00Z", "2026-08-19T12:00:00Z")
                .resolve(now)
                .unwrap(),
            (
                "2026-08-18T12:00:00Z".parse().unwrap(),
                "2026-08-19T12:00:00Z".parse().unwrap(),
            )
        );
        assert_eq!(
            TimeRange::new("now", "now-1h")
                .resolve(now)
                .unwrap_err()
                .to_string(),
            "--from must not be later than --to"
        );
    }
}
