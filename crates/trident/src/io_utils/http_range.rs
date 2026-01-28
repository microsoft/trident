use anyhow::{ensure, Context, Error};

/// Represents a parsed HTTP Content-Range header.
///
/// The Content-Range header format is: "bytes start-end/total"
/// For example: "bytes 0-1023/1024" or "bytes */1024" (for unsatisfied ranges)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRange {
    /// The start byte position (inclusive), if specified
    pub start: Option<u64>,
    /// The end byte position (inclusive), if specified
    pub end: Option<u64>,
    /// The total size of the resource
    pub total: Option<u64>,
}

impl HttpRange {
    /// Parses a Content-Range header value.
    ///
    /// Examples:
    /// - "bytes 0-1023/1024" -> HttpRange { start: Some(0), end: Some(1023), total: Some(1024) }
    /// - "bytes */1024" -> HttpRange { start: None, end: None, total: Some(1024) }
    /// - "bytes 200-999/1000" -> HttpRange { start: Some(200), end: Some(999), total: Some(1000) }
    pub fn parse(s: &str) -> Result<Self, Error> {
        let s = s.trim();

        // Content-Range header should start with "bytes "
        let s = s.strip_prefix("bytes ").with_context(|| {
            format!("Content-Range header must start with 'bytes ', got: '{s}'")
        })?;

        // Split on '/' to separate range from total
        let parts: Vec<&str> = s.split('/').collect();
        ensure!(
            parts.len() == 2,
            "Content-Range header must contain exactly one '/', got: '{s}'"
        );

        let range_part = parts[0];
        let total_part = parts[1];

        // Parse total size
        let total = if total_part == "*" {
            None
        } else {
            Some(
                total_part
                    .parse::<u64>()
                    .with_context(|| format!("Failed to parse total size '{total_part}'"))?,
            )
        };

        // Parse range (start-end or *)
        let (start, end) = if range_part == "*" {
            (None, None)
        } else {
            let range_parts: Vec<&str> = range_part.split('-').collect();
            ensure!(
                range_parts.len() == 2,
                "Range part must contain exactly one '-', got: '{range_part}'"
            );

            let start = range_parts[0]
                .parse::<u64>()
                .with_context(|| format!("Failed to parse start position '{}'", range_parts[0]))?;
            let end = range_parts[1]
                .parse::<u64>()
                .with_context(|| format!("Failed to parse end position '{}'", range_parts[1]))?;

            ensure!(
                start <= end,
                "Start position ({start}) must be less than or equal to end position ({end})"
            );

            (Some(start), Some(end))
        };

        Ok(HttpRange { start, end, total })
    }

    /// Returns the size of the range (number of bytes).
    /// Returns None if the range is unsatisfied (start and end are None).
    pub fn size(&self) -> Option<u64> {
        match (self.start, self.end) {
            (Some(start), Some(end)) => Some(end - start + 1),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_range_parse() {
        // Test standard range
        let range = HttpRange::parse("bytes 0-1023/1024").unwrap();
        assert_eq!(range.start, Some(0));
        assert_eq!(range.end, Some(1023));
        assert_eq!(range.total, Some(1024));
        assert_eq!(range.size(), Some(1024));

        // Test partial range
        let range = HttpRange::parse("bytes 200-999/1000").unwrap();
        assert_eq!(range.start, Some(200));
        assert_eq!(range.end, Some(999));
        assert_eq!(range.total, Some(1000));
        assert_eq!(range.size(), Some(800));

        // Test unsatisfied range
        let range = HttpRange::parse("bytes */1024").unwrap();
        assert_eq!(range.start, None);
        assert_eq!(range.end, None);
        assert_eq!(range.total, Some(1024));
        assert_eq!(range.size(), None);

        // Test unknown total
        let range = HttpRange::parse("bytes 0-499/*").unwrap();
        assert_eq!(range.start, Some(0));
        assert_eq!(range.end, Some(499));
        assert_eq!(range.total, None);
        assert_eq!(range.size(), Some(500));

        // Test with extra whitespace
        let range = HttpRange::parse("  bytes 100-199/500  ").unwrap();
        assert_eq!(range.start, Some(100));
        assert_eq!(range.end, Some(199));
        assert_eq!(range.total, Some(500));
        assert_eq!(range.size(), Some(100));

        // Test single byte range
        let range = HttpRange::parse("bytes 42-42/100").unwrap();
        assert_eq!(range.start, Some(42));
        assert_eq!(range.end, Some(42));
        assert_eq!(range.total, Some(100));
        assert_eq!(range.size(), Some(1));

        // Test error cases
        assert!(HttpRange::parse("0-1023/1024").is_err()); // Missing "bytes "
        assert!(HttpRange::parse("bytes 0-1023").is_err()); // Missing total
        assert!(HttpRange::parse("bytes 0/1024").is_err()); // Missing end
        assert!(HttpRange::parse("bytes abc-def/1024").is_err()); // Invalid numbers
        assert!(HttpRange::parse("bytes 1000-999/1024").is_err()); // Start > end
        assert!(HttpRange::parse("bytes 0-1023/abc").is_err()); // Invalid total
    }
}
