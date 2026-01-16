/// A struct representing a file range in an HTTP request. See:
/// https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Range
///
/// Currently only supports byte ranges. There is no support for multi-part
/// ranges, not suffix byte ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HttpRangeRequest {
    pub start: Option<u64>,
    pub end: Option<u64>,
}

impl HttpRangeRequest {
    /// Converts the HttpRangeRequest to a header value string. Returns None in
    /// cases where no range header would be provided in the request, that is,
    /// when requesting the whole file because both start and end are None.
    pub fn to_header_value_option(self) -> Option<String> {
        Some(match (self.start, self.end) {
            (Some(start), Some(end)) => format!("bytes={}-{}", start, end),
            (Some(start), None) => format!("bytes={}-", start),
            (None, Some(end)) => format!("bytes=0-{}", end),
            (None, None) => return None,
        })
    }

    /// Converts the HttpRangeRequest to a header value string. Returns a default
    /// value of "bytes=-" if no range header would be provided in the request.
    pub fn to_header_value(self) -> String {
        self.to_header_value_option()
            .unwrap_or_else(|| "bytes=0-".into())
    }

    /// Creates a new HttpRangeRequest with the given start and end.
    #[allow(dead_code)]
    pub fn new(start: Option<u64>, end: Option<u64>) -> Self {
        Self { start, end }
    }

    /// Creates a new HttpRangeRequest with the given bounded range.
    pub fn new_bounded(start: u64, end: u64) -> Self {
        Self {
            start: Some(start),
            end: Some(end),
        }
    }

    /// Returns the size of the range in bytes, if it can be determined.
    #[allow(dead_code)] // Used in tests
    pub fn size(&self) -> Option<u64> {
        match (self.start, self.end) {
            // Both start and end are defined
            (Some(start), Some(end)) if end >= start => Some(end - start + 1),

            // Only end is defined, start is 0
            (None, Some(end)) => Some(end + 1),

            // End is not defined, size cannot be determined
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_header_value_option() {
        let range = HttpRangeRequest::new(Some(0), Some(100));
        assert_eq!(
            range.to_header_value_option(),
            Some("bytes=0-99".to_string())
        );

        let range = HttpRangeRequest::new(Some(50), None);
        assert_eq!(
            range.to_header_value_option(),
            Some("bytes=50-".to_string())
        );

        let range = HttpRangeRequest::new(None, Some(200));
        assert_eq!(
            range.to_header_value_option(),
            Some("bytes=0-200".to_string())
        );

        let range = HttpRangeRequest::new(None, None);
        assert_eq!(range.to_header_value_option(), None);
    }

    #[test]
    fn test_to_header_value() {
        let range = HttpRangeRequest::new(Some(0), Some(100));
        assert_eq!(range.to_header_value(), "bytes=0-99".to_string());

        let range = HttpRangeRequest::new(Some(50), None);
        assert_eq!(range.to_header_value(), "bytes=50-".to_string());

        let range = HttpRangeRequest::new(None, Some(200));
        assert_eq!(range.to_header_value(), "bytes=0-200".to_string());

        let range = HttpRangeRequest::new(None, None);
        assert_eq!(range.to_header_value(), "bytes=0-".to_string());
    }
}
