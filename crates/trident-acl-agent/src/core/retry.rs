//! Configurable bounded/unbounded retry, shared by the Kubernetes
//! connect-recovery path (`Orchestrator::get_node_with_retry`) and the watch
//! loop's tolerance for transient stream errors (`Orchestrator::run`).
//!
//! Named after [`MaxTries`] (total attempts, first attempt included) rather
//! than "retries" (additional attempts *after* the first) so that `0` can
//! mean "keep trying forever" without contradicting itself the way "0
//! retries" would - mirrors `wget --tries=0`/`--tries=inf` and systemd's
//! `StartLimitBurst=0`.

use std::{future::Future, str::FromStr, time::Duration};

use anyhow::{anyhow, Error};
use tokio::time;

/// Total connection attempts before giving up, or [`MaxTries::Infinite`] to
/// retry forever. Parsed from an env var via [`FromStr`]: any positive
/// integer is [`MaxTries::Limited`]; `0`, `"infinite"`, or `"forever"`
/// (case-insensitive) is [`MaxTries::Infinite`].
///
/// Invariant: `Limited` is never constructed with `0` - [`FromStr`] maps
/// `"0"` to [`MaxTries::Infinite`] instead (see its docs). Code constructing
/// a `Limited` value directly (e.g. defaults) must uphold the same
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxTries {
    Limited(usize),
    Infinite,
}

impl MaxTries {
    /// True once `tries_so_far` (attempts already made) has used up the
    /// allotment - always `false` for [`MaxTries::Infinite`].
    pub fn is_exhausted(&self, tries_so_far: usize) -> bool {
        match self {
            MaxTries::Infinite => false,
            MaxTries::Limited(max) => tries_so_far >= *max,
        }
    }
}

impl FromStr for MaxTries {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "infinite" | "forever" => Ok(MaxTries::Infinite),
            other => {
                let n: usize = other.parse().map_err(|_| {
                    anyhow!(
                        "invalid max tries {s:?} (expected a non-negative integer, \"infinite\", or \"forever\")"
                    )
                })?;
                Ok(if n == 0 {
                    // "0" means the same thing as "infinite" (wget's own
                    // `--tries=0`/`--tries=inf` convention), rather than "no
                    // attempts at all", which would make the setting useless.
                    MaxTries::Infinite
                } else {
                    MaxTries::Limited(n)
                })
            }
        }
    }
}

/// Distinguishes an error worth retrying from one that never will be, so
/// callers can short-circuit a doomed retry loop (e.g. a Node that's been
/// deleted isn't coming back no matter how many times it's re-read).
pub enum RetryError<E> {
    /// Retrying again would not help - propagate immediately.
    Permanent(E),
    /// Might succeed on a later attempt.
    Transient(E),
}

/// Calls `attempt` up to `max_tries` times (or forever, for
/// [`MaxTries::Infinite`]), sleeping `backoff` between attempts. Returns the
/// first `Ok`, an immediate `Err` for [`RetryError::Permanent`], or the last
/// [`RetryError::Transient`] error once `max_tries` is exhausted.
pub async fn retry<T, E, F, Fut>(
    max_tries: MaxTries,
    backoff: Duration,
    mut attempt: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RetryError<E>>>,
{
    let mut tries = 0usize;
    loop {
        tries += 1;
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(RetryError::Permanent(err)) => return Err(err),
            Err(RetryError::Transient(err)) => {
                if max_tries.is_exhausted(tries) {
                    return Err(err);
                }
                time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn parses_positive_integers_as_limited() {
        assert_eq!("3".parse::<MaxTries>().unwrap(), MaxTries::Limited(3));
        assert_eq!("1".parse::<MaxTries>().unwrap(), MaxTries::Limited(1));
    }

    #[test]
    fn parses_zero_and_words_as_infinite() {
        assert_eq!("0".parse::<MaxTries>().unwrap(), MaxTries::Infinite);
        assert_eq!("infinite".parse::<MaxTries>().unwrap(), MaxTries::Infinite);
        assert_eq!("INFINITE".parse::<MaxTries>().unwrap(), MaxTries::Infinite);
        assert_eq!("forever".parse::<MaxTries>().unwrap(), MaxTries::Infinite);
        assert_eq!("Forever".parse::<MaxTries>().unwrap(), MaxTries::Infinite);
    }

    #[test]
    fn rejects_garbage() {
        assert!("bogus".parse::<MaxTries>().is_err());
        assert!("-1".parse::<MaxTries>().is_err());
        assert!("3.5".parse::<MaxTries>().is_err());
    }

    #[tokio::test]
    async fn succeeds_on_first_try_without_sleeping() {
        let calls = AtomicUsize::new(0);
        let result: Result<u32, ()> = retry(MaxTries::Limited(3), Duration::from_secs(60), || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(42) }
        })
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn permanent_error_stops_immediately() {
        let calls = AtomicUsize::new(0);
        let result: Result<u32, &str> =
            retry(MaxTries::Limited(3), Duration::from_secs(60), || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err(RetryError::Permanent("nope")) }
            })
            .await;
        assert_eq!(result, Err("nope"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn transient_error_retries_until_limited_exhausted() {
        let calls = AtomicUsize::new(0);
        let result: Result<u32, &str> =
            retry(MaxTries::Limited(3), Duration::from_millis(1), || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err(RetryError::Transient("still down")) }
            })
            .await;
        assert_eq!(result, Err("still down"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn transient_error_eventually_succeeds() {
        let calls = AtomicUsize::new(0);
        let result: Result<u32, &str> =
            retry(MaxTries::Limited(5), Duration::from_millis(1), || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        Err(RetryError::Transient("still down"))
                    } else {
                        Ok(7)
                    }
                }
            })
            .await;
        assert_eq!(result, Ok(7));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn infinite_keeps_retrying_past_what_a_limited_count_would_allow() {
        let calls = AtomicUsize::new(0);
        let result: Result<u32, &str> = retry(MaxTries::Infinite, Duration::from_millis(1), || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 50 {
                    Err(RetryError::Transient("still down"))
                } else {
                    Ok(9)
                }
            }
        })
        .await;
        assert_eq!(result, Ok(9));
        assert_eq!(calls.load(Ordering::SeqCst), 51);
    }
}
