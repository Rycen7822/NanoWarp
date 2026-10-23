use std::cell::Cell;
use std::rc::Rc;

use anyhow::anyhow;
use futures::executor::block_on;
use http::StatusCode;

use super::*;
use crate::server::server_api::presigned_upload::HttpStatusError;

#[test]
fn with_retry_succeeds_on_first_attempt() {
    let mut attempts = 0usize;
    let result: Result<u32, &str> = block_on(with_retry(
        "test operation",
        || {
            attempts += 1;
            std::future::ready(Ok(attempts as u32))
        },
        |_err: &&str| true,
        |_delay| std::future::ready(()),
        |_attempts_made| Some(Duration::from_millis(1)),
    ));

    assert_eq!(result, Ok(1));
    assert_eq!(attempts, 1);
}

#[test]
fn with_retry_retries_transient_failures_on_backoff_schedule_then_succeeds() {
    let backoff_schedule = [Duration::from_millis(1), Duration::from_millis(2)];
    let mut attempts = 0usize;
    let mut delays_used = Vec::new();
    let result: Result<u32, &str> = block_on(with_retry(
        "test operation",
        || {
            attempts += 1;
            std::future::ready(if attempts < 3 {
                Err("transient failure")
            } else {
                Ok(attempts as u32)
            })
        },
        |_err: &&str| true,
        |delay| {
            delays_used.push(delay);
            std::future::ready(())
        },
        |attempts_made| backoff_schedule.get(attempts_made).copied(),
    ));

    assert_eq!(result, Ok(3));
    assert_eq!(attempts, 3);
    assert_eq!(delays_used, backoff_schedule);
}

#[test]
fn with_retry_stops_once_backoff_schedule_is_exhausted() {
    let backoff_schedule = [Duration::from_millis(1), Duration::from_millis(2)];
    let mut attempts = 0usize;
    let result: Result<(), &str> = block_on(with_retry(
        "test operation",
        || {
            attempts += 1;
            std::future::ready(Err("persistent failure"))
        },
        |_err: &&str| true,
        |_delay| std::future::ready(()),
        |attempts_made| backoff_schedule.get(attempts_made).copied(),
    ));

    assert_eq!(result, Err("persistent failure"));
    // One initial attempt plus one retry per scheduled backoff delay.
    assert_eq!(attempts, backoff_schedule.len() + 1);
}

#[test]
fn with_retry_fails_fast_when_error_is_not_retryable() {
    let mut attempts = 0usize;
    let result: Result<(), &str> = block_on(with_retry(
        "test operation",
        || {
            attempts += 1;
            std::future::ready(Err("permanent failure"))
        },
        |_err: &&str| false,
        |_delay| std::future::ready(()),
        |_attempts_made| Some(Duration::from_millis(1)),
    ));

    assert_eq!(result, Err("permanent failure"));
    assert_eq!(attempts, 1, "non-retryable errors must not be retried");
}

fn http_err(status: u16) -> anyhow::Error {
    HttpStatusError::new(status, format!("status {status} body")).into()
}

fn graphql_http_err(status: StatusCode) -> anyhow::Error {
    GraphQLError::HttpError {
        status,
        body: format!("status {status} body"),
    }
    .into()
}

#[test]
fn transient_5xx_status_codes_are_retryable() {
    assert!(is_transient_http_error(&http_err(503)));
    assert!(is_transient_http_error(&http_err(500)));
}

#[test]
fn transient_408_and_429_are_retryable() {
    assert!(is_transient_http_error(&http_err(408)));
    assert!(is_transient_http_error(&http_err(429)));
}

#[test]
fn permanent_4xx_status_codes_are_not_retryable() {
    assert!(!is_transient_http_error(&http_err(403)));
    assert!(!is_transient_http_error(&http_err(404)));
    assert!(!is_transient_http_error(&http_err(400)));
}

#[test]
fn errors_without_http_status_are_treated_as_transient() {
    // Network-layer errors (connection reset, timeout, DNS failure) aren't `HttpStatusError`;
    // treat them as transient so the retry loop gives them a chance.
    let err = anyhow!("connection reset by peer");
    assert!(is_transient_http_error(&err));

    let err = anyhow!("Failed to send request: timed out");
    assert!(is_transient_http_error(&err));
}

#[test]
fn graphql_status_classifier_retries_transient_statuses() {
    assert!(is_transient_graphql_or_http_error(&graphql_http_err(
        StatusCode::SERVICE_UNAVAILABLE
    )));
    assert!(is_transient_graphql_or_http_error(&graphql_http_err(
        StatusCode::REQUEST_TIMEOUT
    )));
    assert!(is_transient_graphql_or_http_error(&graphql_http_err(
        StatusCode::TOO_MANY_REQUESTS
    )));
}

#[test]
fn graphql_status_classifier_fails_fast_on_permanent_statuses() {
    assert!(!is_transient_graphql_or_http_error(&graphql_http_err(
        StatusCode::UNAUTHORIZED
    )));
    assert!(!is_transient_graphql_or_http_error(&graphql_http_err(
        StatusCode::FORBIDDEN
    )));
    assert!(!is_transient_graphql_or_http_error(&graphql_http_err(
        StatusCode::NOT_FOUND
    )));
}

#[test]
fn graphql_status_classifier_fails_fast_without_typed_transport_error() {
    // GraphQL user-facing errors are converted to plain anyhow errors at the operation
    // layer, so GraphQL retry call sites should not treat unknown errors as transient.
    let err = anyhow!("user-facing GraphQL error");
    assert!(!is_transient_graphql_or_http_error(&err));
}

#[test]
fn retry_loop_succeeds_on_first_attempt() {
    let attempts = Rc::new(Cell::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<()> = block_on(with_bounded_retry("test retry", || {
        attempts_clone.set(attempts_clone.get() + 1);
        async { Ok(()) }
    }));
    result.unwrap();
    assert_eq!(attempts.get(), 1);
}

#[test]
fn retry_loop_retries_transient_and_eventually_succeeds() {
    let attempts = Rc::new(Cell::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<u32> = block_on(with_bounded_retry("test retry", || {
        let n = attempts_clone.get() + 1;
        attempts_clone.set(n);
        async move { if n < 2 { Err(http_err(503)) } else { Ok(n) } }
    }));
    assert_eq!(result.unwrap(), 2);
    assert_eq!(attempts.get(), 2);
}

#[test]
fn retry_loop_stops_at_max_attempts_on_persistent_transient() {
    let attempts = Rc::new(Cell::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<()> = block_on(with_bounded_retry("test retry", || {
        attempts_clone.set(attempts_clone.get() + 1);
        async { Err(http_err(503)) }
    }));
    assert!(result.is_err());
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}

#[test]
fn retry_loop_fails_fast_on_permanent_error() {
    let attempts = Rc::new(Cell::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<()> = block_on(with_bounded_retry("test retry", || {
        attempts_clone.set(attempts_clone.get() + 1);
        async { Err(http_err(403)) }
    }));
    assert!(result.is_err());
    assert_eq!(attempts.get(), 1, "permanent errors should not retry");
}

#[test]
fn with_bounded_retry_using_applies_custom_classifier() {
    // An error with no typed HTTP/GraphQL cause is transient under the default
    // `is_transient_http_error` classifier but permanent under
    // `is_transient_graphql_or_http_error`. `with_bounded_retry_using` must apply the
    // classifier it's given, not the default.
    let attempts = Rc::new(Cell::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<()> = block_on(with_bounded_retry_using(
        "test retry",
        MAX_ATTEMPTS,
        is_transient_graphql_or_http_error,
        || {
            attempts_clone.set(attempts_clone.get() + 1);
            async { Err(anyhow!("user-facing GraphQL error")) }
        },
    ));
    assert!(result.is_err());
    assert_eq!(
        attempts.get(),
        1,
        "an untyped error should fail fast under is_transient_graphql_or_http_error"
    );
}

#[test]
fn with_bounded_retry_using_honors_custom_attempt_budget() {
    // Deliberately different from the shared `MAX_ATTEMPTS` (3), and kept small so the
    // test only pays for one backoff.
    const CUSTOM_MAX_ATTEMPTS: usize = 2;
    let attempts = Rc::new(Cell::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<()> = block_on(with_bounded_retry_using(
        "test retry",
        CUSTOM_MAX_ATTEMPTS,
        is_transient_graphql_or_http_error,
        || {
            attempts_clone.set(attempts_clone.get() + 1);
            async { Err(graphql_http_err(StatusCode::SERVICE_UNAVAILABLE)) }
        },
    ));
    assert!(result.is_err());
    assert_eq!(
        attempts.get(),
        CUSTOM_MAX_ATTEMPTS,
        "should retry up to the caller-supplied budget, not the shared MAX_ATTEMPTS"
    );
}
