use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ValidCardinality {
    min_count: Option<usize>,
    max_count: Option<usize>,
}

impl ValidCardinality {
    pub fn new_zero() -> Self {
        Self {
            min_count: Some(0),
            max_count: Some(0),
        }
    }

    pub fn new_exact(v: usize) -> Self {
        Self {
            min_count: Some(v),
            max_count: Some(v),
        }
    }

    pub fn new_at_least(v: usize) -> Self {
        Self {
            min_count: Some(v),
            max_count: None,
        }
    }

    pub fn new_at_most(v: usize) -> Self {
        Self {
            min_count: None,
            max_count: Some(v),
        }
    }

    pub fn new_range(start: usize, end: usize) -> Self {
        Self {
            min_count: Some(start),
            max_count: Some(end),
        }
    }

    pub fn min(&self) -> Option<usize> {
        self.min_count
    }

    pub fn max(&self) -> Option<usize> {
        self.max_count
    }

    pub fn contains(&self, v: usize) -> bool {
        match (self.min_count, self.max_count) {
            (Some(start), Some(end)) => start <= v && v <= end,
            (Some(start), None) => start <= v,
            (None, Some(end)) => v <= end,
            (None, None) => true,
        }
    }
}

impl std::fmt::Display for ValidCardinality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.min_count, self.max_count) {
            (Some(start), Some(end)) if start == end => write!(f, "exactly {}", start),
            (Some(start), Some(end)) => write!(f, "between {} and {}", start, end),
            (Some(start), None) => write!(f, "at least {}", start),
            (None, Some(end)) => write!(f, "at most {}", end),
            (None, None) => write!(f, "any or none"),
        }
    }
}
