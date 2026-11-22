use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// A random token with a readable prefix, such as `sb_3f9a...`.
pub fn random_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

/// Unix milliseconds as RFC 3339 in UTC, such as `2026-11-22T14:05:09.120Z`.
pub fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms.rem_euclid(1000)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_rfc3339() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            super::rfc3339(1_795_356_309_120),
            "2026-11-22T14:05:09.120Z"
        );
        assert_eq!(super::rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
