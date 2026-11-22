/// Checks RFC 3339 syntax and calendar ranges without changing the input.
/// Leap seconds are accepted at UTC month-end 23:59;
/// no historical leap-second table is consulted.
pub fn is_valid_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    fn number(b: &[u8]) -> Option<u32> {
        b.iter().try_fold(0u32, |n, c| if c.is_ascii_digit() { Some(n * 10 + (c - b'0') as u32) } else { None })
    }
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') || b[13] != b':' || b[16] != b':' { return false; }
    let (Some(y), Some(m), Some(d), Some(h), Some(min), Some(sec)) = (number(&b[0..4]), number(&b[5..7]), number(&b[8..10]), number(&b[11..13]), number(&b[14..16]), number(&b[17..19])) else { return false; };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m { 1 | 3 | 5 | 7 | 8 | 10 | 12 => 31, 4 | 6 | 9 | 11 => 30, 2 => if leap { 29 } else { 28 }, _ => return false };
    if d == 0 || d > days || h > 23 || min > 59 || sec > 60 { return false; }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) { i += 1; }
        if i == start { return false; }
    }
    let offset = match b.get(i) {
        Some(b'Z' | b'z') if i + 1 == b.len() => 0,
        Some(b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            match (number(&b[i+1..i+3]), number(&b[i+4..i+6])) {
                (Some(hour @ 0..=23), Some(minute @ 0..=59)) => {
                    let value = (hour * 60 + minute) as i32;
                    if b[i] == b'+' { value } else { -value }
                }
                _ => return false,
            }
        }
        _ => return false,
    };
    if sec != 60 { return true; }
    let utc_minutes = (h * 60 + min) as i32 - offset;
    if utc_minutes.rem_euclid(1440) != 1439 { return false; }
    match utc_minutes.div_euclid(1440) {
        -1 => d == 1,
        0 => d == days,
        1 => d + 1 == days,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_valid_timestamp;
    #[test]
    fn boundaries() {
        for s in ["2000-02-29T23:59:59Z", "2016-12-31T23:59:60Z", "2017-01-01T05:29:60+05:30", "2024-02-29t12:34:56.123456789+23:59", "2020-01-01T00:00:00-00:00"] { assert!(is_valid_timestamp(s), "{s}"); }
        for s in ["2017-01-01T05:59:60+05:30", "2016-12-30T23:59:60Z", "2024-02-29T23:58:60Z", "1900-02-29T00:00:00Z", "2023-02-29T00:00:00Z", "2024-04-31T00:00:00Z", "2024-01-01T24:00:00Z", "2024-01-01T12:58:60Z", "2024-01-01T00:00:61Z", "2024-01-01T00:00:00", "2024-01-01T00:00:00.Z", "2024-01-01T00:00:00+24:00", "2024-01-01T00:00:00+01:60", "2024-01-01T00:00:00Zx"] { assert!(!is_valid_timestamp(s), "{s}"); }
    }
}
