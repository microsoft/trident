use std::ops::RangeFrom;

pub struct IdGenerator {
    name: String,
    range: RangeFrom<u64>,
}

impl IdGenerator {
    pub fn new(name: String) -> Self {
        Self { name, range: 0.. }
    }

    pub fn next(&mut self) -> String {
        format!("{}-{}", self.name, self.range.next().unwrap())
    }
}
