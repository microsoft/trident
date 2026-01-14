/// A struct representing a file range in an HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HttpRangeRequest {
    pub start: Option<u64>,
    pub end: Option<u64>,
}

impl HttpRangeRequest {
    /// Converts the HttpRangeRequest to a header value string. Returns None in
    /// cases where no range header would be provided in the request, that is,
    /// when requesting the whole file because both start and end are None.
    pub fn to_header_value(self) -> Option<String> {
        Some(match (self.start, self.end) {
            (Some(start), Some(end)) => format!("bytes={}-{}", start, end - 1),
            (Some(start), None) => format!("bytes={}-", start),
            (None, Some(end)) => format!("bytes=-{}", end),
            (None, None) => return None,
        })
    }

    pub fn new(start: Option<u64>, end: Option<u64>) -> Self {
        Self { start, end }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_header_value() {
        let range = HttpRangeRequest::new(Some(0), Some(100));
        assert_eq!(range.to_header_value(), Some("bytes=0-99".to_string()));

        let range = HttpRangeRequest::new(Some(50), None);
        assert_eq!(range.to_header_value(), Some("bytes=50-".to_string()));

        let range = HttpRangeRequest::new(None, Some(200));
        assert_eq!(range.to_header_value(), Some("bytes=-200".to_string()));

        let range = HttpRangeRequest::new(None, None);
        assert_eq!(range.to_header_value(), None);
    }
}
