use chrono::Duration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub most_likely: Duration,
    pub optimistic: Duration,
    pub pessimistic: Duration,
}

impl Estimate {
    pub fn new(most_likely: Duration) -> Self {
        Self {
            most_likely,
            optimistic: most_likely,
            pessimistic: most_likely,
        }
    }
    pub fn from_mop(most_likely: Duration, optimistic: Duration, pessimistic: Duration) -> Result<Self, String> {
        if optimistic > most_likely || most_likely > pessimistic {
            return Err("Optimistic time must be less than or equal to Most Likely time, which must be less than or equal to Pessimistic time.".to_string());
        }
        if optimistic.num_minutes() <= 0 || most_likely.num_minutes() <= 0 || pessimistic.num_minutes() <= 0 {
            return Err("All estimates must be greater than zero.".to_string());
        }
        Ok(Self { most_likely, optimistic, pessimistic })
    }
    pub fn mean(&self) -> Duration {
        (self.optimistic + self.most_likely * 4 + self.pessimistic) / 6
    }
    pub fn stddev(&self) -> Duration {
        (self.pessimistic - self.optimistic) / 6
    }
    pub fn variance_minutes(&self) -> i64 {
        let stddev = self.stddev().num_minutes();
        stddev * stddev
    }
}

impl std::ops::Add for Estimate {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            most_likely: self.most_likely + other.most_likely,
            optimistic: self.optimistic + other.optimistic,
            pessimistic: self.pessimistic + other.pessimistic,
        }
    }
}
impl std::ops::Sub for Estimate {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            most_likely: self.most_likely - other.most_likely,
            optimistic: self.optimistic - other.optimistic,
            pessimistic: self.pessimistic - other.pessimistic,
        }
    }
}
