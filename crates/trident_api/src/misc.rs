use std::ops::RangeFrom;

pub struct IdGenerator {
    name: String,
    range: RangeFrom<u64>,
}

impl IdGenerator {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            range: 0..,
        }
    }

    pub fn new_with_start(name: impl Into<String>, start: u64) -> Self {
        Self {
            name: name.into(),
            range: start..,
        }
    }

    pub fn next_id(&mut self) -> String {
        format!("{}-{}", self.name, self.range.next().unwrap())
    }
}
