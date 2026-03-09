use anyhow::{Context as AnyhowContext, Result};
use boa_engine::string::JsString;
use boa_engine::{Context, JsValue, NativeFunction, Source};
use glob::Pattern;
use log::{error, info};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct PacEngine {
    senders: Vec<mpsc::Sender<PacRequest>>,
    next_worker: Arc<AtomicUsize>,
}

struct PacRequest {
    url: String,
    host: String,
    respond_to: oneshot::Sender<Result<String>>,
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // The glob crate fails to parse patterns with more than two consecutive asterisks
    // (e.g. `***`) returning a PatternError ("wildcards are either regular `*` or recursive `**`").
    // We collapse consecutive asterisks to `*` before parsing.
    let mut collapsed_pattern = String::with_capacity(pattern.len());
    let mut last_was_star = false;
    for c in pattern.chars() {
        if c == '*' {
            if !last_was_star {
                collapsed_pattern.push(c);
                last_was_star = true;
            }
        } else {
            collapsed_pattern.push(c);
            last_was_star = false;
        }
    }

    Pattern::new(&collapsed_pattern)
        .map(|p| p.matches(text))
        .unwrap_or(false)
}

/// Resolve a hostname to its first IPv4 address string, or return the input
/// unchanged if it is already a valid IPv4 address.  Returns `None` when
/// resolution fails.
fn resolve_host_to_ipv4(host: &str) -> Option<String> {
    // Fast-path: already an IPv4 literal
    if host.parse::<Ipv4Addr>().is_ok() {
        return Some(host.to_string());
    }
    format!("{}:0", host)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| {
            addrs.find_map(|a| match a.ip() {
                IpAddr::V4(v4) => Some(v4.to_string()),
                _ => None,
            })
        })
}

/// Best-effort detection of the machine's outbound IPv4 address.
/// Opens a UDP socket towards a public DNS server (without actually sending
/// data) and reads the local address the OS selected.  Falls back to
/// 127.0.0.1 when the network is unreachable.
fn detect_my_ip_address() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|sock| {
            // connect() on a UDP socket doesn't send anything – it just
            // lets the OS pick a source address.
            sock.connect("8.8.8.8:53")?;
            sock.local_addr()
        })
        .ok()
        .and_then(|addr| match addr.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() => Some(v4.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// Map a three-letter weekday abbreviation to a 0-based index (SUN=0 … SAT=6).
fn weekday_index(s: &str) -> Option<u32> {
    match s.to_uppercase().as_str() {
        "SUN" => Some(0),
        "MON" => Some(1),
        "TUE" => Some(2),
        "WED" => Some(3),
        "THU" => Some(4),
        "FRI" => Some(5),
        "SAT" => Some(6),
        _ => None,
    }
}

/// Map a three-letter month abbreviation to a 1-based index (JAN=1 … DEC=12).
fn month_index(s: &str) -> Option<u32> {
    match s.to_uppercase().as_str() {
        "JAN" => Some(1),
        "FEB" => Some(2),
        "MAR" => Some(3),
        "APR" => Some(4),
        "MAY" => Some(5),
        "JUN" => Some(6),
        "JUL" => Some(7),
        "AUG" => Some(8),
        "SEP" => Some(9),
        "OCT" => Some(10),
        "NOV" => Some(11),
        "DEC" => Some(12),
        _ => None,
    }
}

/// Check whether `val` is inside a possibly-wrapping range `[lo, hi]` (inclusive).
fn in_range(val: u32, lo: u32, hi: u32) -> bool {
    if lo <= hi {
        val >= lo && val <= hi
    } else {
        // Wrapping range, e.g. FRI..MON means FRI,SAT,SUN,MON
        val >= lo || val <= hi
    }
}

/// Extract current local or GMT time components as
/// (wday 0-6, day 1-31, month 1-12, year, hour, min, sec).
fn now_components(use_gmt: bool) -> (u32, u32, u32, u32, u32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if use_gmt {
        let (year, month, day, hour, min, sec) = unix_to_calendar_utc(secs);
        let wday = weekday_from_date(year, month, day);
        (wday, day, month, year, hour, min, sec)
    } else {
        // Use libc localtime for local timezone
        #[cfg(unix)]
        {
            let tm = unsafe {
                let t = secs as libc::time_t;
                let mut result: libc::tm = std::mem::zeroed();
                libc::localtime_r(&t, &mut result);
                result
            };
            (
                tm.tm_wday as u32,
                tm.tm_mday as u32,
                (tm.tm_mon + 1) as u32,
                (tm.tm_year + 1900) as u32,
                tm.tm_hour as u32,
                tm.tm_min as u32,
                tm.tm_sec as u32,
            )
        }
        #[cfg(not(unix))]
        {
            // Fallback: use UTC on non-unix
            let (year, month, day, hour, min, sec) = unix_to_calendar_utc(secs);
            let wday = weekday_from_date(year, month, day);
            (wday, day, month, year, hour, min, sec)
        }
    }
}

/// Convert unix timestamp to (year, month 1-12, day 1-31, hour, min, sec) in UTC.
fn unix_to_calendar_utc(secs: i64) -> (u32, u32, u32, u32, u32, u32) {
    // Algorithm from Howard Hinnant's civil_from_days
    let s = secs.rem_euclid(86400) as u32;
    let z = secs.div_euclid(86400) + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if m <= 2 { y + 1 } else { y };

    (yr as u32, m, d, s / 3600, (s % 3600) / 60, s % 60)
}

/// Zeller-like weekday: 0=SUN, 1=MON, ..., 6=SAT for a given date.
fn weekday_from_date(year: u32, month: u32, day: u32) -> u32 {
    // Tomohiko Sakamoto's algorithm
    let t = [0u32, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year;
    if month < 3 {
        y -= 1;
    }
    (y + y / 4 - y / 100 + y / 400 + t[(month - 1) as usize] + day) % 7
}

fn register_pac_functions(context: &mut Context) {
    // ---------------------------------------------------------------
    // alert(message) — PAC spec logging function.
    // Logs the message; prevents JS runtime errors in PAC scripts that
    // call alert() for debugging.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("alert"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let msg = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|| {
                    args.first()
                        .map(|v| v.display().to_string())
                        .unwrap_or_default()
                });
            info!("PAC-alert: {}", msg);
            Ok(JsValue::undefined())
        }),
    );

    // ---------------------------------------------------------------
    // dnsResolve(host) — resolves hostname to dotted IPv4 string.
    // Returns null if resolution fails (per spec behaviour in browsers).
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("dnsResolve"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            match resolve_host_to_ipv4(&host) {
                Some(ip) => Ok(JsValue::from(JsString::from(ip))),
                None => Ok(JsValue::null()),
            }
        }),
    );

    // ---------------------------------------------------------------
    // myIpAddress() — returns this machine's outbound IPv4 address.
    // We detect the IP once at registration time and store it as a JS
    // global constant so the closure can use from_fn_ptr (no captures).
    // ---------------------------------------------------------------
    let my_ip = detect_my_ip_address();
    let my_ip_js = JsString::from(my_ip.as_str());
    context
        .global_object()
        .set(
            JsString::from("__ferrovela_my_ip"),
            JsValue::from(my_ip_js),
            false,
            context,
        )
        .ok();
    let _ = context.register_global_callable(
        JsString::from("myIpAddress"),
        0,
        NativeFunction::from_fn_ptr(|_, _, ctx| {
            let global = ctx.global_object();
            let val = global
                .get(JsString::from("__ferrovela_my_ip"), ctx)
                .unwrap_or_else(|_| JsValue::from(JsString::from("127.0.0.1")));
            Ok(val)
        }),
    );

    // ---------------------------------------------------------------
    // shExpMatch(str, shExp) — shell expression / glob matching.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("shExpMatch"),
        2,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let str_val = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let pattern = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let matched = glob_match(&pattern, &str_val);
            Ok(JsValue::from(matched))
        }),
    );

    // ---------------------------------------------------------------
    // isPlainHostName(host) — true if hostname has no dots.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("isPlainHostName"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(!host.contains('.')))
        }),
    );

    // ---------------------------------------------------------------
    // dnsDomainIs(host, domain) — true if host ends with domain.
    //
    // Per the MDN spec examples:
    //   dnsDomainIs("www.mozilla.org", ".mozilla.org") => true
    //   dnsDomainIs("www", ".mozilla.org")             => false
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("dnsDomainIs"),
        2,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let domain = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(host.ends_with(&domain)))
        }),
    );

    // ---------------------------------------------------------------
    // localHostOrDomainIs(host, hostdom) — true if:
    //   1. Exact match (host == hostdom), OR
    //   2. host is an unqualified name (no dots) and hostdom starts
    //      with "host."
    //
    // MDN spec examples:
    //   localHostOrDomainIs("www.mozilla.org", "www.mozilla.org") => true
    //   localHostOrDomainIs("www",             "www.mozilla.org") => true
    //   localHostOrDomainIs("www.google.com",  "www.mozilla.org") => false
    //   localHostOrDomainIs("home.mozilla.org","www.mozilla.org") => false
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("localHostOrDomainIs"),
        2,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let hostdom = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            // Exact match
            if host == hostdom {
                return Ok(JsValue::from(true));
            }
            // Unqualified host (no dots) matches if hostdom starts with "host."
            if !host.contains('.') && hostdom.starts_with(&format!("{}.", host)) {
                return Ok(JsValue::from(true));
            }
            Ok(JsValue::from(false))
        }),
    );

    // ---------------------------------------------------------------
    // isResolvable(host) — true if DNS can resolve the host.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("isResolvable"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let resolvable = format!("{}:0", host)
                .to_socket_addrs()
                .map(|mut addrs| addrs.next().is_some())
                .unwrap_or(false);
            Ok(JsValue::from(resolvable))
        }),
    );

    // ---------------------------------------------------------------
    // isInNet(host, pattern, mask)
    // If host is a hostname it is resolved first.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("isInNet"),
        3,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let pattern = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let mask = args
                .get(2)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let resolve_ip = |h: &str| -> Option<u32> {
                if let Ok(ip) = h.parse::<Ipv4Addr>() {
                    return Some(u32::from(ip));
                }
                format!("{}:0", h)
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut addrs| {
                        addrs.find_map(|a| match a {
                            std::net::SocketAddr::V4(v4) => Some(u32::from(*v4.ip())),
                            _ => None,
                        })
                    })
            };

            let result = (|| -> Option<bool> {
                let host_ip = resolve_ip(&host)?;
                let pattern_ip = pattern.parse::<Ipv4Addr>().ok()?;
                let mask_ip = mask.parse::<Ipv4Addr>().ok()?;
                let pattern_int = u32::from(pattern_ip);
                let mask_int = u32::from(mask_ip);
                Some((host_ip & mask_int) == (pattern_int & mask_int))
            })()
            .unwrap_or(false);
            Ok(JsValue::from(result))
        }),
    );

    // ---------------------------------------------------------------
    // dnsDomainLevels(host) — number of dots in hostname.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("dnsDomainLevels"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let levels = host.matches('.').count() as i32;
            Ok(JsValue::from(levels))
        }),
    );

    // ---------------------------------------------------------------
    // convert_addr(ipaddr) — dotted IP to 32-bit integer.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("convert_addr"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let addr = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let result = addr
                .parse::<Ipv4Addr>()
                .map(|ip| u32::from(ip) as f64)
                .unwrap_or(0.0);
            Ok(JsValue::from(result))
        }),
    );

    // ---------------------------------------------------------------
    // weekdayRange(wd1 [, wd2 [, "GMT"]])
    //
    // If only wd1 is given, returns true on that weekday.
    // If wd1 and wd2 are given, returns true if current weekday is
    // in the (possibly wrapping) range [wd1, wd2].
    // If the last argument is "GMT", comparison uses UTC.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("weekdayRange"),
        3,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let str_args: Vec<String> = args
                .iter()
                .filter_map(|v| v.as_string().map(|s| s.to_std_string_escaped()))
                .collect();

            let use_gmt = str_args
                .last()
                .map(|s| s.eq_ignore_ascii_case("GMT"))
                .unwrap_or(false);
            let non_gmt: Vec<&str> = if use_gmt {
                str_args[..str_args.len() - 1]
                    .iter()
                    .map(|s| s.as_str())
                    .collect()
            } else {
                str_args.iter().map(|s| s.as_str()).collect()
            };

            let (wday, _, _, _, _, _, _) = now_components(use_gmt);

            let result = match non_gmt.len() {
                1 => weekday_index(non_gmt[0])
                    .map(|d| wday == d)
                    .unwrap_or(false),
                2 => match (weekday_index(non_gmt[0]), weekday_index(non_gmt[1])) {
                    (Some(d1), Some(d2)) => in_range(wday, d1, d2),
                    _ => false,
                },
                _ => false,
            };
            Ok(JsValue::from(result))
        }),
    );

    // ---------------------------------------------------------------
    // dateRange(...)
    //
    // Supports the common calling conventions:
    //   dateRange(day)
    //   dateRange(day1, day2)
    //   dateRange(month)
    //   dateRange(month1, month2)
    //   dateRange(year)
    //   dateRange(year1, year2)
    //   dateRange(day1, month1, day2, month2)
    //   dateRange(month1, year1, month2, year2)
    //   dateRange(day1, month1, year1, day2, month2, year2)
    //   Any of the above with an optional trailing "GMT" parameter.
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("dateRange"),
        7,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let str_args: Vec<String> = args
                .iter()
                .filter_map(|v| {
                    if let Some(s) = v.as_string() {
                        Some(s.to_std_string_escaped())
                    } else {
                        v.as_number().map(|n| (n as i64).to_string())
                    }
                })
                .collect();

            let use_gmt = str_args
                .last()
                .map(|s| s.eq_ignore_ascii_case("GMT"))
                .unwrap_or(false);
            let params: Vec<&str> = if use_gmt {
                str_args[..str_args.len() - 1]
                    .iter()
                    .map(|s| s.as_str())
                    .collect()
            } else {
                str_args.iter().map(|s| s.as_str()).collect()
            };

            let (_, cur_day, cur_month, cur_year, _, _, _) = now_components(use_gmt);

            // Helper: classify a token as month name, day (1-31), or year (>31)
            let classify = |s: &str| -> (Option<u32>, Option<u32>, Option<u32>) {
                // month name?
                if let Some(m) = month_index(s) {
                    return (None, Some(m), None);
                }
                // number
                if let Ok(n) = s.parse::<u32>() {
                    if (1..=31).contains(&n) {
                        return (Some(n), None, None); // day
                    }
                    return (None, None, Some(n)); // year
                }
                (None, None, None)
            };

            let result = match params.len() {
                // Single value
                1 => {
                    let (d, m, y) = classify(params[0]);
                    if let Some(day) = d {
                        cur_day == day
                    } else if let Some(month) = m {
                        cur_month == month
                    } else if let Some(year) = y {
                        cur_year == year
                    } else {
                        false
                    }
                }
                // Two values: day-day, month-month, or year-year
                2 => {
                    let (d1, m1, y1) = classify(params[0]);
                    let (d2, m2, y2) = classify(params[1]);
                    if let (Some(a), Some(b)) = (d1, d2) {
                        in_range(cur_day, a, b)
                    } else if let (Some(a), Some(b)) = (m1, m2) {
                        in_range(cur_month, a, b)
                    } else if let (Some(a), Some(b)) = (y1, y2) {
                        in_range(cur_year, a, b)
                    } else {
                        false
                    }
                }
                // Four values: day1,month1,day2,month2 or month1,year1,month2,year2
                4 => {
                    let (d1, m1, _) = classify(params[0]);
                    let (_, m1b, y1) = classify(params[1]);
                    let (d2, m2, _) = classify(params[2]);
                    let (_, m2b, y2) = classify(params[3]);

                    match (d1, m1b, d2, m2b, m1, y1, m2, y2) {
                        (Some(dv1), Some(mv1), Some(dv2), Some(mv2), _, _, _, _) => {
                            // day1, month1, day2, month2
                            let start = mv1 * 100 + dv1;
                            let end = mv2 * 100 + dv2;
                            let cur = cur_month * 100 + cur_day;
                            in_range(cur, start, end)
                        }
                        (_, _, _, _, Some(mv1), Some(yv1), Some(mv2), Some(yv2)) => {
                            // month1, year1, month2, year2
                            let start = yv1 * 100 + mv1;
                            let end = yv2 * 100 + mv2;
                            let cur = cur_year * 100 + cur_month;
                            in_range(cur, start, end)
                        }
                        _ => false,
                    }
                }
                // Six values: day1, month1, year1, day2, month2, year2
                6 => {
                    let (d1, _, _) = classify(params[0]);
                    let (_, m1, _) = classify(params[1]);
                    let (_, _, y1) = classify(params[2]);
                    let (d2, _, _) = classify(params[3]);
                    let (_, m2, _) = classify(params[4]);
                    let (_, _, y2) = classify(params[5]);

                    if let (Some(d1v), Some(m1v), Some(y1v), Some(d2v), Some(m2v), Some(y2v)) =
                        (d1, m1, y1, d2, m2, y2)
                    {
                        let start = y1v * 10000 + m1v * 100 + d1v;
                        let end = y2v * 10000 + m2v * 100 + d2v;
                        let cur = cur_year * 10000 + cur_month * 100 + cur_day;
                        in_range(cur, start, end)
                    } else {
                        false
                    }
                }
                _ => false,
            };
            Ok(JsValue::from(result))
        }),
    );

    // ---------------------------------------------------------------
    // timeRange(...)
    //
    // Supports:
    //   timeRange(hour)
    //   timeRange(hour1, hour2)
    //   timeRange(hour1, min1, hour2, min2)
    //   timeRange(hour1, min1, sec1, hour2, min2, sec2)
    //   Any with optional trailing "GMT".
    // ---------------------------------------------------------------
    let _ = context.register_global_callable(
        JsString::from("timeRange"),
        7,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let nums: Vec<String> = args
                .iter()
                .filter_map(|v| {
                    if let Some(s) = v.as_string() {
                        Some(s.to_std_string_escaped())
                    } else {
                        v.as_number().map(|n| (n as i64).to_string())
                    }
                })
                .collect();

            let use_gmt = nums
                .last()
                .map(|s| s.eq_ignore_ascii_case("GMT"))
                .unwrap_or(false);
            let params: Vec<&str> = if use_gmt {
                nums[..nums.len() - 1].iter().map(|s| s.as_str()).collect()
            } else {
                nums.iter().map(|s| s.as_str()).collect()
            };

            let (_, _, _, _, cur_h, cur_m, cur_s) = now_components(use_gmt);

            let parse = |s: &str| -> u32 { s.parse::<u32>().unwrap_or(0) };

            let result = match params.len() {
                // timeRange(hour) => true during that whole hour
                1 => cur_h == parse(params[0]),
                // timeRange(hour1, hour2)
                2 => in_range(cur_h, parse(params[0]), parse(params[1])),
                // timeRange(hour1, min1, hour2, min2)
                4 => {
                    let cur_val = cur_h * 60 + cur_m;
                    let lo = parse(params[0]) * 60 + parse(params[1]);
                    let hi = parse(params[2]) * 60 + parse(params[3]);
                    in_range(cur_val, lo, hi)
                }
                // timeRange(hour1, min1, sec1, hour2, min2, sec2)
                6 => {
                    let cur_val = cur_h * 3600 + cur_m * 60 + cur_s;
                    let lo = parse(params[0]) * 3600 + parse(params[1]) * 60 + parse(params[2]);
                    let hi = parse(params[3]) * 3600 + parse(params[4]) * 60 + parse(params[5]);
                    in_range(cur_val, lo, hi)
                }
                _ => false,
            };
            Ok(JsValue::from(result))
        }),
    );
}

impl PacEngine {
    pub async fn new(pac_url_or_path: &str) -> Result<Self> {
        let script = if pac_url_or_path.starts_with("http") {
            // PAC files must always be fetched with a DIRECT connection (no proxy),
            // otherwise we'd have a circular dependency: needing the proxy config
            // to fetch the file that provides proxy config.
            let client = reqwest::Client::builder()
                .no_proxy()
                .build()
                .context("Failed to build HTTP client for PAC fetch")?;
            client
                .get(pac_url_or_path)
                .send()
                .await?
                .text()
                .await
                .context("Failed to fetch PAC file")?
        } else {
            fs::read_to_string(pac_url_or_path).context("Failed to read PAC file")?
        };

        // Determine number of workers based on available cores (fallback to 4)
        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let mut senders = Vec::with_capacity(num_workers);

        for _ in 0..num_workers {
            let (tx, mut rx) = mpsc::channel::<PacRequest>(32);
            senders.push(tx);
            let script_clone = script.clone();

            // Boa JS engine uses deep recursion for parsing/evaluation;
            // the default thread stack size can cause stack overflows on
            // complex PAC scripts, so we allocate 8 MB per worker thread.
            thread::Builder::new()
                .name("pac-worker".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    let mut context = Context::default();

                    register_pac_functions(&mut context);

                    if let Err(e) = context.eval(Source::from_bytes(&script_clone)) {
                        error!("Failed to evaluate PAC script: {}", e);
                    }

                    while let Some(req) = rx.blocking_recv() {
                        let global_obj = context.global_object();

                        let result = (|| -> Result<String> {
                            let func_name = JsString::from("FindProxyForURL");
                            let func = global_obj
                                .get(func_name, &mut context)
                                .map_err(|e| anyhow::anyhow!("JS Error: {}", e))?;

                            if !func.is_callable() {
                                return Err(anyhow::anyhow!("FindProxyForURL is not defined"));
                            }
                            let args = [
                                JsValue::from(JsString::from(req.url)),
                                JsValue::from(JsString::from(req.host)),
                            ];
                            let res = func
                                .as_callable()
                                .unwrap()
                                .call(&JsValue::undefined(), &args, &mut context)
                                .map_err(|e| anyhow::anyhow!("JS Error: {}", e))?;

                            res.as_string()
                                .map(|s| s.to_std_string_escaped())
                                .ok_or_else(|| {
                                    anyhow::anyhow!("FindProxyForURL returned non-string")
                                })
                        })();

                        let _ = req.respond_to.send(result);
                    }
                })
                .context("Failed to spawn PAC worker thread")?;
        }

        Ok(PacEngine {
            senders,
            next_worker: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub async fn find_proxy(&self, url: &str, host: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let req = PacRequest {
            url: url.to_string(),
            host: host.to_string(),
            respond_to: tx,
        };

        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.senders.len();

        self.senders[worker_idx]
            .send(req)
            .await
            .map_err(|_| anyhow::anyhow!("PAC thread dead"))?;

        match rx.await {
            Ok(res) => res,
            Err(_) => Err(anyhow::anyhow!("PAC thread dropped channel")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source};

    /// Helper: create a Boa context with all PAC functions registered.
    fn pac_context() -> Context {
        let mut ctx = Context::default();
        register_pac_functions(&mut ctx);
        ctx
    }

    /// Helper: evaluate a JS expression and return the JsValue.
    fn eval(ctx: &mut Context, code: &str) -> JsValue {
        ctx.eval(Source::from_bytes(code))
            .unwrap_or_else(|e| panic!("JS eval failed for `{}`: {}", code, e))
    }

    // =================================================================
    // glob_match (internal helper)
    // =================================================================

    #[test]
    fn test_glob_match_cases() {
        let cases = vec![
            // Exact match
            ("abc", "abc", true),
            ("abc", "def", false),
            // Wildcard (*)
            ("*", "anything", true),
            ("*", "", true),
            ("abc*", "abcdef", true),
            ("*def", "abcdef", true),
            ("ab*ef", "abcdef", true),
            ("a*c", "abc", true),
            ("a*c", "abbc", true),
            ("*bc*", "abcdef", true),
            // Question mark (?)
            ("?", "a", true),
            ("?", "", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("a?c", "abbc", false),
            // Mixed
            ("?b*", "abc", true),
            ("*b?", "abc", true),
            // Edge cases
            ("", "", true),
            ("", "a", false),
            ("a", "", false),
            ("*******", "a", true),
        ];

        for (pattern, text, expected) in cases {
            assert_eq!(
                glob_match(pattern, text),
                expected,
                "Pattern: '{}', Text: '{}'",
                pattern,
                text
            );
        }
    }

    // =================================================================
    // alert()
    // =================================================================

    #[test]
    fn test_alert_does_not_throw() {
        let mut ctx = pac_context();
        let res = ctx.eval(Source::from_bytes("alert('hello from PAC')"));
        assert!(res.is_ok(), "alert() must not throw");
    }

    #[test]
    fn test_alert_with_non_string() {
        let mut ctx = pac_context();
        let res = ctx.eval(Source::from_bytes("alert(42)"));
        assert!(res.is_ok(), "alert() with a number must not throw");
    }

    #[test]
    fn test_alert_returns_undefined() {
        let mut ctx = pac_context();
        let val = eval(&mut ctx, "alert('test')");
        assert!(val.is_undefined());
    }

    // =================================================================
    // isPlainHostName()
    // =================================================================

    #[test]
    fn test_is_plain_host_name() {
        let mut ctx = pac_context();

        assert_eq!(
            eval(&mut ctx, "isPlainHostName('example.com')").as_boolean(),
            Some(false)
        );
        assert_eq!(
            eval(&mut ctx, "isPlainHostName('localhost')").as_boolean(),
            Some(true)
        );
        assert_eq!(
            eval(&mut ctx, "isPlainHostName('www')").as_boolean(),
            Some(true)
        );
        assert_eq!(
            eval(&mut ctx, "isPlainHostName('a.b.c')").as_boolean(),
            Some(false)
        );
    }

    // =================================================================
    // dnsDomainIs()
    // =================================================================

    #[test]
    fn test_dns_domain_is() {
        let mut ctx = pac_context();

        // From MDN spec
        assert_eq!(
            eval(&mut ctx, "dnsDomainIs('www.mozilla.org', '.mozilla.org')").as_boolean(),
            Some(true)
        );
        assert_eq!(
            eval(&mut ctx, "dnsDomainIs('www', '.mozilla.org')").as_boolean(),
            Some(false)
        );
        // Additional cases
        assert_eq!(
            eval(&mut ctx, "dnsDomainIs('www.mozilla.org', 'mozilla.org')").as_boolean(),
            Some(true)
        );
        // "mozilla.org" does NOT end with ".mozilla.org" — the leading dot matters
        assert_eq!(
            eval(&mut ctx, "dnsDomainIs('mozilla.org', '.mozilla.org')").as_boolean(),
            Some(false)
        );
    }

    // =================================================================
    // localHostOrDomainIs()
    // =================================================================

    #[test]
    fn test_local_host_or_domain_is_exact_match() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(
                &mut ctx,
                "localHostOrDomainIs('www.mozilla.org', 'www.mozilla.org')"
            )
            .as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_local_host_or_domain_is_unqualified_match() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "localHostOrDomainIs('www', 'www.mozilla.org')").as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_local_host_or_domain_is_domain_mismatch() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(
                &mut ctx,
                "localHostOrDomainIs('www.google.com', 'www.mozilla.org')"
            )
            .as_boolean(),
            Some(false)
        );
    }

    #[test]
    fn test_local_host_or_domain_is_hostname_mismatch() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(
                &mut ctx,
                "localHostOrDomainIs('home.mozilla.org', 'www.mozilla.org')"
            )
            .as_boolean(),
            Some(false)
        );
    }

    #[test]
    fn test_local_host_or_domain_is_fqdn_does_not_match_prefix() {
        // A fully-qualified host must NOT match just because hostdom starts
        // with it followed by a dot — that path is only for unqualified names.
        let mut ctx = pac_context();
        assert_eq!(
            eval(
                &mut ctx,
                "localHostOrDomainIs('home.mozilla.org', 'home.mozilla.org.evil.com')"
            )
            .as_boolean(),
            Some(false)
        );
    }

    // =================================================================
    // dnsDomainLevels()
    // =================================================================

    #[test]
    fn test_dns_domain_levels() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "dnsDomainLevels('www')").as_number(),
            Some(0.0)
        );
        assert_eq!(
            eval(&mut ctx, "dnsDomainLevels('mozilla.org')").as_number(),
            Some(1.0)
        );
        assert_eq!(
            eval(&mut ctx, "dnsDomainLevels('www.mozilla.org')").as_number(),
            Some(2.0)
        );
    }

    // =================================================================
    // shExpMatch()
    // =================================================================

    #[test]
    fn test_sh_exp_match() {
        let mut ctx = pac_context();

        // From MDN spec examples
        assert_eq!(
            eval(
                &mut ctx,
                "shExpMatch('http://home.netscape.com/people/ari/index.html', '*/ari/*')"
            )
            .as_boolean(),
            Some(true)
        );
        assert_eq!(
            eval(
                &mut ctx,
                "shExpMatch('http://home.netscape.com/people/montulli/index.html', '*/ari/*')"
            )
            .as_boolean(),
            Some(false)
        );
    }

    // =================================================================
    // dnsResolve()
    // =================================================================

    #[test]
    fn test_dns_resolve_ip_literal() {
        let mut ctx = pac_context();
        let val = eval(&mut ctx, "dnsResolve('127.0.0.1')");
        assert_eq!(
            val.as_string().map(|s| s.to_std_string_escaped()),
            Some("127.0.0.1".to_string())
        );
    }

    #[test]
    fn test_dns_resolve_localhost() {
        let mut ctx = pac_context();
        let val = eval(&mut ctx, "dnsResolve('localhost')");
        let resolved = val
            .as_string()
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        assert_eq!(
            resolved, "127.0.0.1",
            "Expected dnsResolve('localhost') == '127.0.0.1', got '{}'",
            resolved
        );
    }

    #[test]
    fn test_dns_resolve_unresolvable_returns_null() {
        let mut ctx = pac_context();
        let val = eval(&mut ctx, "dnsResolve('this.host.does.not.exist.invalid')");
        assert!(
            val.is_null(),
            "dnsResolve on an unresolvable host should return null"
        );
    }

    // =================================================================
    // myIpAddress()
    // =================================================================

    #[test]
    fn test_my_ip_address_returns_valid_ipv4() {
        let mut ctx = pac_context();
        let val = eval(&mut ctx, "myIpAddress()");
        let ip_str = val
            .as_string()
            .map(|s| s.to_std_string_escaped())
            .expect("myIpAddress() should return a string");
        assert!(
            ip_str.parse::<Ipv4Addr>().is_ok(),
            "myIpAddress() returned '{}' which is not a valid IPv4 address",
            ip_str
        );
    }

    // =================================================================
    // isResolvable()
    // =================================================================

    #[test]
    fn test_is_resolvable_localhost() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "isResolvable('localhost')").as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_is_resolvable_invalid() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "isResolvable('this.host.does.not.exist.invalid')").as_boolean(),
            Some(false)
        );
    }

    // =================================================================
    // isInNet()
    // =================================================================

    #[test]
    fn test_is_in_net_ip_literal() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(
                &mut ctx,
                "isInNet('192.168.1.100', '192.168.1.0', '255.255.255.0')"
            )
            .as_boolean(),
            Some(true)
        );
        assert_eq!(
            eval(
                &mut ctx,
                "isInNet('10.0.0.1', '192.168.1.0', '255.255.255.0')"
            )
            .as_boolean(),
            Some(false)
        );
    }

    #[test]
    fn test_is_in_net_localhost() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "isInNet('127.0.0.1', '127.0.0.0', '255.0.0.0')").as_boolean(),
            Some(true)
        );
    }

    // =================================================================
    // convert_addr()
    // =================================================================

    #[test]
    fn test_convert_addr() {
        let mut ctx = pac_context();
        // 10.0.0.1 = 0x0A000001 = 167772161
        assert_eq!(
            eval(&mut ctx, "convert_addr('10.0.0.1')").as_number(),
            Some(167772161.0)
        );
        // 255.255.255.255 = 4294967295
        assert_eq!(
            eval(&mut ctx, "convert_addr('255.255.255.255')").as_number(),
            Some(4294967295.0)
        );
        // Invalid
        assert_eq!(
            eval(&mut ctx, "convert_addr('not_an_ip')").as_number(),
            Some(0.0)
        );
    }

    // =================================================================
    // weekdayRange()
    // =================================================================

    #[test]
    fn test_weekday_range_current_day_matches() {
        let mut ctx = pac_context();
        // SUN..SAT covers all days
        assert_eq!(
            eval(&mut ctx, "weekdayRange('SUN', 'SAT')").as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_weekday_range_single_day() {
        let mut ctx = pac_context();
        let (wday, _, _, _, _, _, _) = now_components(false);
        let day_name = match wday {
            0 => "SUN",
            1 => "MON",
            2 => "TUE",
            3 => "WED",
            4 => "THU",
            5 => "FRI",
            6 => "SAT",
            _ => unreachable!(),
        };
        let code = format!("weekdayRange('{}')", day_name);
        assert_eq!(
            eval(&mut ctx, &code).as_boolean(),
            Some(true),
            "weekdayRange('{}') should be true today",
            day_name
        );
    }

    // =================================================================
    // dateRange()
    // =================================================================

    #[test]
    fn test_date_range_current_year() {
        let mut ctx = pac_context();
        let (_, _, _, year, _, _, _) = now_components(false);
        let code = format!("dateRange({})", year);
        assert_eq!(
            eval(&mut ctx, &code).as_boolean(),
            Some(true),
            "dateRange({}) should be true this year",
            year
        );
    }

    #[test]
    fn test_date_range_wrong_year() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "dateRange(1999)").as_boolean(),
            Some(false),
            "dateRange(1999) should be false (unless time travel)"
        );
    }

    #[test]
    fn test_date_range_current_day() {
        let mut ctx = pac_context();
        let (_, day, _, _, _, _, _) = now_components(false);
        let code = format!("dateRange({})", day);
        assert_eq!(
            eval(&mut ctx, &code).as_boolean(),
            Some(true),
            "dateRange({}) should be true today",
            day
        );
    }

    #[test]
    fn test_date_range_full_year_range() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "dateRange(1990, 2099)").as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_date_range_month_range_all() {
        let mut ctx = pac_context();
        assert_eq!(
            eval(&mut ctx, "dateRange('JAN', 'DEC')").as_boolean(),
            Some(true)
        );
    }

    // =================================================================
    // timeRange()
    // =================================================================

    #[test]
    fn test_time_range_all_hours() {
        let mut ctx = pac_context();
        assert_eq!(eval(&mut ctx, "timeRange(0, 23)").as_boolean(), Some(true));
    }

    #[test]
    fn test_time_range_current_hour() {
        let mut ctx = pac_context();
        let (_, _, _, _, hour, _, _) = now_components(false);
        let code = format!("timeRange({})", hour);
        assert_eq!(
            eval(&mut ctx, &code).as_boolean(),
            Some(true),
            "timeRange({}) should match current hour",
            hour
        );
    }

    // =================================================================
    // Internal helpers
    // =================================================================

    #[test]
    fn test_weekday_index() {
        assert_eq!(weekday_index("SUN"), Some(0));
        assert_eq!(weekday_index("mon"), Some(1));
        assert_eq!(weekday_index("Sat"), Some(6));
        assert_eq!(weekday_index("XYZ"), None);
    }

    #[test]
    fn test_month_index() {
        assert_eq!(month_index("JAN"), Some(1));
        assert_eq!(month_index("dec"), Some(12));
        assert_eq!(month_index("Jul"), Some(7));
        assert_eq!(month_index("XYZ"), None);
    }

    #[test]
    fn test_in_range_normal() {
        assert!(in_range(3, 1, 5));
        assert!(in_range(1, 1, 5));
        assert!(in_range(5, 1, 5));
        assert!(!in_range(0, 1, 5));
        assert!(!in_range(6, 1, 5));
    }

    #[test]
    fn test_in_range_wrapping() {
        // FRI(5)..MON(1): should match FRI,SAT,SUN,MON
        assert!(in_range(5, 5, 1)); // FRI
        assert!(in_range(6, 5, 1)); // SAT
        assert!(in_range(0, 5, 1)); // SUN
        assert!(in_range(1, 5, 1)); // MON
        assert!(!in_range(2, 5, 1)); // TUE
        assert!(!in_range(3, 5, 1)); // WED
        assert!(!in_range(4, 5, 1)); // THU
    }

    #[test]
    fn test_unix_to_calendar_utc_epoch() {
        let (y, m, d, h, mi, s) = unix_to_calendar_utc(0);
        assert_eq!((y, m, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn test_unix_to_calendar_utc_known_date() {
        // 2025-06-15 15:10:45 UTC = 1750000245
        let (y, m, d, h, mi, s) = unix_to_calendar_utc(1750000245);
        assert_eq!(y, 2025);
        assert_eq!(m, 6);
        assert_eq!(d, 15);
        assert_eq!(h, 15);
        assert_eq!(mi, 10);
        assert_eq!(s, 45);
    }

    #[test]
    fn test_weekday_from_date_known() {
        // 2025-01-01 was a Wednesday (3)
        assert_eq!(weekday_from_date(2025, 1, 1), 3);
        // 1970-01-01 was a Thursday (4)
        assert_eq!(weekday_from_date(1970, 1, 1), 4);
    }

    #[test]
    fn test_detect_my_ip_address_returns_valid_ipv4() {
        let ip = detect_my_ip_address();
        assert!(
            ip.parse::<Ipv4Addr>().is_ok(),
            "detect_my_ip_address() returned '{}' which is not valid IPv4",
            ip
        );
    }

    #[test]
    fn test_resolve_host_to_ipv4_ip_literal() {
        assert_eq!(
            resolve_host_to_ipv4("10.0.0.1"),
            Some("10.0.0.1".to_string())
        );
    }

    #[test]
    fn test_resolve_host_to_ipv4_localhost() {
        let result = resolve_host_to_ipv4("localhost");
        assert_eq!(result, Some("127.0.0.1".to_string()));
    }

    #[test]
    fn test_resolve_host_to_ipv4_unresolvable() {
        assert_eq!(
            resolve_host_to_ipv4("this.host.does.not.exist.invalid"),
            None
        );
    }

    // =================================================================
    // End-to-end: complete PAC scripts evaluated through the engine
    // =================================================================

    #[test]
    fn test_full_pac_script_direct() {
        let mut ctx = pac_context();
        let pac = r#"
            function FindProxyForURL(url, host) {
                if (isPlainHostName(host)) return "DIRECT";
                if (dnsDomainIs(host, ".local")) return "DIRECT";
                return "PROXY proxy.corp:8080; DIRECT";
            }
        "#;
        ctx.eval(Source::from_bytes(pac)).unwrap();

        let res = ctx
            .eval(Source::from_bytes(
                "FindProxyForURL('http://intranet/', 'intranet')",
            ))
            .unwrap();
        assert_eq!(
            res.as_string().map(|s| s.to_std_string_escaped()),
            Some("DIRECT".to_string())
        );
    }

    #[test]
    fn test_full_pac_script_proxy() {
        let mut ctx = pac_context();
        let pac = r#"
            function FindProxyForURL(url, host) {
                if (isPlainHostName(host)) return "DIRECT";
                if (dnsDomainIs(host, ".local")) return "DIRECT";
                return "PROXY proxy.corp:8080; DIRECT";
            }
        "#;
        ctx.eval(Source::from_bytes(pac)).unwrap();

        let res = ctx
            .eval(Source::from_bytes(
                "FindProxyForURL('http://www.example.com/', 'www.example.com')",
            ))
            .unwrap();
        assert_eq!(
            res.as_string().map(|s| s.to_std_string_escaped()),
            Some("PROXY proxy.corp:8080; DIRECT".to_string())
        );
    }

    #[test]
    fn test_full_pac_script_with_alert() {
        let mut ctx = pac_context();
        let pac = r#"
            function FindProxyForURL(url, host) {
                alert("Evaluating: " + host);
                if (shExpMatch(host, "*.example.com")) {
                    return "PROXY p1:80";
                }
                return "DIRECT";
            }
        "#;
        ctx.eval(Source::from_bytes(pac)).unwrap();

        let res = ctx
            .eval(Source::from_bytes(
                "FindProxyForURL('http://www.example.com/', 'www.example.com')",
            ))
            .unwrap();
        assert_eq!(
            res.as_string().map(|s| s.to_std_string_escaped()),
            Some("PROXY p1:80".to_string())
        );
    }

    #[test]
    fn test_full_pac_script_subnet_based() {
        let mut ctx = pac_context();
        let pac = r#"
            function FindProxyForURL(url, host) {
                if (isInNet(host, "127.0.0.0", "255.0.0.0")) {
                    return "DIRECT";
                }
                return "PROXY upstream:3128";
            }
        "#;
        ctx.eval(Source::from_bytes(pac)).unwrap();

        let res = ctx
            .eval(Source::from_bytes(
                "FindProxyForURL('http://127.0.0.1/', '127.0.0.1')",
            ))
            .unwrap();
        assert_eq!(
            res.as_string().map(|s| s.to_std_string_escaped()),
            Some("DIRECT".to_string())
        );
    }

    #[test]
    fn test_full_pac_script_domain_levels() {
        let mut ctx = pac_context();
        let pac = r#"
            function FindProxyForURL(url, host) {
                if (dnsDomainLevels(host) > 1) {
                    return "PROXY proxy:80";
                }
                return "DIRECT";
            }
        "#;
        ctx.eval(Source::from_bytes(pac)).unwrap();

        // 0 dots
        let res = ctx
            .eval(Source::from_bytes(
                "FindProxyForURL('http://intranet/', 'intranet')",
            ))
            .unwrap();
        assert_eq!(
            res.as_string().map(|s| s.to_std_string_escaped()),
            Some("DIRECT".to_string())
        );

        // 2 dots
        let res = ctx
            .eval(Source::from_bytes(
                "FindProxyForURL('http://www.mozilla.org/', 'www.mozilla.org')",
            ))
            .unwrap();
        assert_eq!(
            res.as_string().map(|s| s.to_std_string_escaped()),
            Some("PROXY proxy:80".to_string())
        );
    }
}
