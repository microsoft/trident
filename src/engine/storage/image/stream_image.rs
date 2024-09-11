use std::time::Duration;

use anyhow::{Context, Error};
use log::error;
use reqwest::{blocking::Response, StatusCode, Url};

pub const GET_MAX_RETRIES: u8 = 25;
pub const GET_TIMEOUT_SECS: u64 = 600;

/// Perform a GET request with retries and exponential backoff.
///
/// The function will do a GET request to the given URL and return the response
/// if the status code is OK.
///
/// `max_retries` is the number of *additional* attempts to make after the first.
/// Passing 0 will make a single attempt, passing 1 will make at most two attempts, etc.
///
/// `timeout` is the timeout for the blocking get request.
///
/// The backoff is exponential, starting at 500ms and doubling each time, up to a maximum of 16s.
pub(crate) fn exponential_backoff_get(
    url: &Url,
    max_retries: u8,
    timeout: Duration,
) -> Result<Box<Response>, Error> {
    let mut counter = 0u8;
    let client = reqwest::blocking::ClientBuilder::new()
        .timeout(timeout)
        .build()
        .context("Failed to create HTTP client")?;
    loop {
        // Try to execute the GET request, if it works and we get a 200 OK,
        // return the response immediately. Otherwise, store the error and
        // continue the loop.
        let err: Error = match client.get(url.clone()).send().context("Failed to GET") {
            Ok(response) if matches!(response.status(), StatusCode::OK) => {
                // On success, exit exponential_backoff_get() by returning the response
                return Ok(Box::new(response));
            }
            // Otherwise store the error to report it later and continue the loop
            Ok(response) => anyhow::anyhow!("Failed to GET with status {}", response.status()),
            Err(e) => e,
        };

        // Check if we reached the limit
        if counter >= max_retries {
            return Err(err).context(format!(
                "Failed to GET from {url} after {} attempts.",
                (counter as u16) + 1, // change to u16 to avoid overflow
            ));
        }

        counter += 1;

        // Calculate exponential backoff.
        // After 16 seconds we cap the backoff and retry until we reach the limit
        // of retries.
        // Because it's just a couple values it's easier to just hardcode it
        // than spending the extra ticks to calculate a power of 2.
        //
        // backoff = 0.5 * 2^(counter - 1) until 6 retries. Post that it is a constant 16 seconds.
        // 0.5, 1, 2, 4, 8, 16, 16, 16, 16, 16, ...
        let backoff = Duration::from_millis(match counter {
            1 => 500,
            2 => 1000,
            3 => 2000,
            4 => 4000,
            5 => 8000,
            6 => 16000,
            _ => 16000,
        });

        // Log the error and backoff
        error!(
            "Failed to GET from {}: {}. Retrying in {:4.1} seconds.",
            url,
            err,
            backoff.as_secs_f32()
        );
        std::thread::sleep(backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    #[test]
    fn test_exponential_backoff() {
        let fake_url = Url::try_from("http://127.0.0.1:3030").unwrap();
        let start = Instant::now();
        let result = exponential_backoff_get(&fake_url, 2, Duration::from_secs(1));
        let duration = start.elapsed();

        // 2 retries means 3 attempts:
        //(attempt) + 0.5s delay + (attempt) + 1s delay + (attempt)
        //
        // Because these are blocking calls the total duration should be at
        // least 1.5s
        assert!(
            duration >= Duration::from_millis(1500),
            "Duration was {:?}",
            duration
        );

        assert!(result.is_err(), "Expected error, got {:?}", result);

        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to GET from {} after {} attempts.", fake_url, 3),
            "Error doesn't match expected"
        );
    }
}
