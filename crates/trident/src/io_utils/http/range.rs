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
    pub fn to_header_value_option(self) -> Option<String> {
        Some(match (self.start, self.end) {
            (Some(start), Some(end)) => format!("bytes={}-{}", start, end - 1),
            (Some(start), None) => format!("bytes={}-", start),
            (None, Some(end)) => format!("bytes=-{}", end),
            (None, None) => return None,
        })
    }

    /// Converts the HttpRangeRequest to a header value string. Returns a default
    /// value of "bytes=-" if no range header would be provided in the request.
    pub fn to_header_value(self) -> String {
        self.to_header_value_option()
            .unwrap_or_else(|| "bytes=-".into())
    }

    /// Creates a new HttpRangeRequest with the given start and end.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_header_value() {
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
            Some("bytes=-200".to_string())
        );

        let range = HttpRangeRequest::new(None, None);
        assert_eq!(range.to_header_value_option(), None);
    }
}
