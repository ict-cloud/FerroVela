use ferrovela::proxy::connect::find_subsequence;

#[test]
fn test_find_subsequence() {
    // Basic match
    let haystack = b"hello world";
    let needle = b"world";
    assert_eq!(find_subsequence(haystack, needle), Some(6));

    // Start match
    let haystack = b"hello world";
    let needle = b"hello";
    assert_eq!(find_subsequence(haystack, needle), Some(0));

    // End match
    let haystack = b"hello world";
    let needle = b"d";
    assert_eq!(find_subsequence(haystack, needle), Some(10));

    // No match
    let haystack = b"hello world";
    let needle = b"foo";
    assert_eq!(find_subsequence(haystack, needle), None);

    // Multiple matches (should find first)
    let haystack = b"banana";
    let needle = b"na";
    assert_eq!(find_subsequence(haystack, needle), Some(2));

    // Empty haystack
    let haystack = b"";
    let needle = b"a";
    assert_eq!(find_subsequence(haystack, needle), None);

    // Needle larger than haystack
    let haystack = b"hi";
    let needle = b"hello";
    assert_eq!(find_subsequence(haystack, needle), None);
}

#[test]
#[should_panic]
fn test_find_subsequence_empty_needle_panic() {
    let haystack = b"hello world";
    let needle = b"";
    find_subsequence(haystack, needle);
}
