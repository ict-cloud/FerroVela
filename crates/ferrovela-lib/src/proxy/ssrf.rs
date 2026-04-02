use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns `true` if `target` (`host:port`) is a private, loopback, or otherwise
/// non-routable address that should be blocked when `allow_private_ips` is `false`.
///
/// Only IP-literal hosts are evaluated.  Hostnames are **not** resolved:
/// DNS resolution is async, subject to TOCTOU, and easily bypassed; network-layer
/// controls should handle hostname-based SSRF.
pub fn is_private_target(target: &str) -> bool {
    let host = match target.rsplit_once(':') {
        Some((h, _)) => h,
        None => target,
    };
    // Strip IPv6 brackets: [::1]:443 → ::1
    let host = host.trim_start_matches('[').trim_end_matches(']');

    match host.parse::<IpAddr>() {
        Ok(ip) => is_private_ip(ip),
        Err(_) => false, // hostname — cannot determine without DNS
    }
}

/// If `bytes` look like the beginning of an HTTP CONNECT request, extract and
/// return the `host:port` target from the request line.
///
/// Designed to work on a partial peek buffer: the target must be fully present
/// in `bytes`, but nothing else needs to be.  If parsing fails the caller
/// should conservatively allow the connection.
pub fn connect_target_from_peek(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?;
    // Take text up to the first newline (or end-of-buffer if no newline yet).
    let first_line = s.lines().next()?;
    let mut parts = first_line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    Some(parts.next()?.to_string())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => is_private_v6(v6),
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()       // 127.0.0.0/8
        || ip.is_private() // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
        || ip.is_link_local() // 169.254.0.0/16
        || ip.is_broadcast() // 255.255.255.255
        || ip.is_unspecified() // 0.0.0.0
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()             // ::1
        || ip.is_unspecified()   // ::
        || is_ipv6_unique_local(ip) // fc00::/7
        || is_ipv6_link_local(ip) // fe80::/10
}

/// fc00::/7 — unique local (RFC 4193).  `Ipv6Addr::is_unique_local` is not yet
/// stable in the Rust standard library.
fn is_ipv6_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// fe80::/10 — link-local (RFC 4291).  `Ipv6Addr::is_unicast_link_local` is
/// not yet stable in the Rust standard library.
fn is_ipv6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_private_target ─────────────────────────────────────────────────────

    #[test]
    fn public_ipv4_is_allowed() {
        assert!(!is_private_target("1.1.1.1:443"));
        assert!(!is_private_target("8.8.8.8:53"));
        assert!(!is_private_target("203.0.113.5:80"));
    }

    #[test]
    fn loopback_is_blocked() {
        assert!(is_private_target("127.0.0.1:80"));
        assert!(is_private_target("127.1.2.3:8080"));
        assert!(is_private_target("[::1]:443"));
    }

    #[test]
    fn rfc1918_is_blocked() {
        assert!(is_private_target("10.0.0.1:80"));
        assert!(is_private_target("10.255.255.255:443"));
        assert!(is_private_target("172.16.0.1:80"));
        assert!(is_private_target("172.31.255.255:80"));
        assert!(is_private_target("192.168.1.1:80"));
        assert!(is_private_target("192.168.255.255:443"));
    }

    #[test]
    fn link_local_is_blocked() {
        assert!(is_private_target("169.254.0.1:80"));
        assert!(is_private_target("169.254.169.254:80")); // AWS metadata endpoint
        assert!(is_private_target("[fe80::1]:80"));
    }

    #[test]
    fn unspecified_and_broadcast_are_blocked() {
        assert!(is_private_target("0.0.0.0:80"));
        assert!(is_private_target("255.255.255.255:80"));
        assert!(is_private_target("[::]:80"));
    }

    #[test]
    fn ipv6_unique_local_is_blocked() {
        assert!(is_private_target("[fc00::1]:443"));
        assert!(is_private_target("[fd00::1]:443"));
    }

    #[test]
    fn hostname_is_allowed() {
        // Hostnames cannot be evaluated without DNS; we fail open.
        assert!(!is_private_target("internal.corp:80"));
        assert!(!is_private_target("localhost:80"));
    }

    #[test]
    fn target_without_port_is_handled() {
        assert!(is_private_target("127.0.0.1"));
        assert!(!is_private_target("1.1.1.1"));
    }

    // ── connect_target_from_peek ──────────────────────────────────────────────

    #[test]
    fn extracts_connect_target() {
        let bytes = b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        assert_eq!(
            connect_target_from_peek(bytes),
            Some("example.com:443".to_string())
        );
    }

    #[test]
    fn extracts_connect_target_from_partial_buffer() {
        // Peek may not contain the full request — only the first line matters.
        let bytes = b"CONNECT 192.168.1.1:443 HTTP/1.1\r\n";
        assert_eq!(
            connect_target_from_peek(bytes),
            Some("192.168.1.1:443".to_string())
        );
    }

    #[test]
    fn returns_none_for_non_connect() {
        assert_eq!(
            connect_target_from_peek(b"GET http://example.com/ HTTP/1.1\r\n"),
            None
        );
    }

    #[test]
    fn returns_none_for_empty() {
        assert_eq!(connect_target_from_peek(b""), None);
    }

    #[test]
    fn case_insensitive_method() {
        let bytes = b"connect 10.0.0.1:80 HTTP/1.1\r\n";
        assert_eq!(
            connect_target_from_peek(bytes),
            Some("10.0.0.1:80".to_string())
        );
    }
}
