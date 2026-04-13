/// Find the first occurrence of `needle` in `haystack`.
///
/// Uses the SIMD-accelerated `memchr::memmem` searcher rather than a
/// byte-at-a-time `windows().position()` loop.
#[inline]
pub fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack, needle)
}

/// Parse the `Content-Length` header value from a raw header block.
///
/// Returns 0 if the header is absent or malformed.
#[inline]
pub fn parse_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        if line.len() >= 15 && line.as_bytes()[..15].eq_ignore_ascii_case(b"content-length:") {
            if let Some(val) = line.split(':').nth(1) {
                return val.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Find the value of the first header whose name matches `key`
/// (case-insensitive), returning a borrowed `&str` into `headers`.
///
/// Returns `None` if the header is absent.
#[inline]
pub fn find_header_value<'a>(headers: &'a str, key: &str) -> Option<&'a str> {
    let key_len = key.len();
    for line in headers.lines() {
        if line.len() > key_len
            && line.as_bytes()[key_len] == b':'
            && line[..key_len].eq_ignore_ascii_case(key)
        {
            return Some(line[key_len + 1..].trim());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_subsequence() {
        let haystack = b"hello world";
        assert_eq!(find_subsequence(haystack, b"world"), Some(6));
        assert_eq!(find_subsequence(haystack, b"foo"), None);
        assert_eq!(find_subsequence(haystack, b"hello"), Some(0));
    }

    #[test]
    fn test_parse_content_length() {
        let headers = "Content-Type: text/plain\r\nContent-Length: 42\r\n";
        assert_eq!(parse_content_length(headers), 42);

        let headers_mixed = "content-length: 100\r\n";
        assert_eq!(parse_content_length(headers_mixed), 100);

        let headers_none = "Host: example.com\r\n";
        assert_eq!(parse_content_length(headers_none), 0);
    }

    #[test]
    fn test_find_header_value() {
        let headers = "Proxy-Authenticate: Basic realm=\"proxy\"\r\nConnection: keep-alive\r\n";
        assert_eq!(
            find_header_value(headers, "Proxy-Authenticate"),
            Some("Basic realm=\"proxy\"")
        );
        assert_eq!(find_header_value(headers, "connection"), Some("keep-alive"));
        assert_eq!(find_header_value(headers, "foo"), None);
    }
}
