//! Deterministic retry timing for the transfer queue.
//!
//! This module does not decide whether an operation is safe to retry. That
//! decision is typed separately by `RetryDisposition`. It only supplies a
//! small bounded backoff once an executor has explicitly returned
//! `SafeToRetry`.

use std::time::Duration;

pub const TRANSFER_RETRY_BASE_DELAY: Duration = Duration::from_millis(250);
pub const TRANSFER_RETRY_MAX_DELAY: Duration = Duration::from_secs(2);

/// Return bounded exponential backoff for a *next* total-attempt number.
///
/// Attempt 1 is the initial execution and has no retry delay. With the current
/// maximum of three total attempts, attempt 2 waits 250 ms and attempt 3 waits
/// 500 ms. The function is generic for future bounded policies and never
/// returns a delay larger than `TRANSFER_RETRY_MAX_DELAY`.
pub fn retry_backoff(next_attempt: u8, max_total_attempts: u8) -> Option<Duration> {
    if next_attempt < 2 || next_attempt > max_total_attempts {
        return None;
    }

    let exponent = u32::from(next_attempt.saturating_sub(2)).min(16);
    let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
    Some(
        TRANSFER_RETRY_BASE_DELAY
            .saturating_mul(multiplier)
            .min(TRANSFER_RETRY_MAX_DELAY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_total_attempts_have_two_bounded_delays() {
        assert_eq!(retry_backoff(1, 3), None);
        assert_eq!(retry_backoff(2, 3), Some(Duration::from_millis(250)));
        assert_eq!(retry_backoff(3, 3), Some(Duration::from_millis(500)));
        assert_eq!(retry_backoff(4, 3), None);
    }

    #[test]
    fn future_attempt_counts_remain_capped() {
        assert_eq!(retry_backoff(6, 8), Some(TRANSFER_RETRY_MAX_DELAY));
        assert_eq!(retry_backoff(u8::MAX, u8::MAX), Some(TRANSFER_RETRY_MAX_DELAY));
    }
}
