//! Watches an image being sent, so a download that the display drops half way is noticed. The display
//! can't report that itself: a wake that fails mid-download only says so on a later one, if at all.
//!
//! The body is sent in chunks. If the connection goes away before the last one has been taken, the
//! body is dropped unfinished, and that is logged with who it was, how far it got and how long it took.
//! Only what the server can see is claimed: a small file can sit entirely in the kernel's send buffer, so
//! a display that stops reading a 150 KB image may still look complete from here.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use futures_util::Stream;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::display::ImageFormat;

const CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Complete,
    /// The client went away after `sent` of the bytes had been handed to the connection.
    Aborted {
        sent: usize,
    },
}

struct Tracked<F: FnOnce(Outcome, Duration)> {
    data: Bytes,
    sent: usize,
    started: Instant,
    done: Option<F>,
}

impl<F: FnOnce(Outcome, Duration) + Unpin> Stream for Tracked<F> {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.sent >= self.data.len() {
            if let Some(done) = self.done.take() {
                done(Outcome::Complete, self.started.elapsed());
            }
            return Poll::Ready(None);
        }
        let end = (self.sent + CHUNK).min(self.data.len());
        let chunk = self.data.slice(self.sent..end);
        self.sent = end;
        Poll::Ready(Some(Ok(chunk)))
    }
}

impl<F: FnOnce(Outcome, Duration)> Drop for Tracked<F> {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            // With a known length the server stops reading the body after the last byte, so it is dropped
            // without being asked for its end: everything handed over is a complete transfer.
            let outcome = if self.sent >= self.data.len() {
                Outcome::Complete
            } else {
                Outcome::Aborted { sent: self.sent }
            };
            done(outcome, self.started.elapsed());
        }
    }
}

/// A body that calls `done` once, when it has been sent in full or dropped before that.
fn tracked(data: Vec<u8>, done: impl FnOnce(Outcome, Duration) + Unpin + Send + 'static) -> Body {
    Body::from_stream(Tracked {
        data: Bytes::from(data),
        sent: 0,
        started: Instant::now(),
        done: Some(done),
    })
}

/// The image as a body that logs if the display drops the connection before the end.
pub(super) fn watched(data: Vec<u8>, device: Option<DeviceId>, format: ImageFormat) -> Body {
    let total = data.len();
    tracked(data, move |outcome, took| {
        if let Outcome::Aborted { sent } = outcome {
            let who = device.map_or_else(
                || "A client".to_owned(),
                |device| format!("Display {device}"),
            );
            log::warn!(
                "{who} dropped the {} image after {sent} of {total} bytes ({} ms)",
                format.extension(),
                took.as_millis()
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::to_bytes;
    use futures_util::StreamExt;

    use super::*;

    fn recorder() -> (
        Arc<Mutex<Vec<Outcome>>>,
        impl FnOnce(Outcome, Duration) + Unpin + Send + 'static,
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&seen);
        (seen, move |outcome, _| writer.lock().unwrap().push(outcome))
    }

    #[tokio::test]
    async fn a_body_read_to_the_end_is_complete_and_unchanged() {
        let data: Vec<u8> = (0..CHUNK * 3 + 5).map(|i| i as u8).collect();
        let (seen, done) = recorder();
        let body = tracked(data.clone(), done);
        assert_eq!(
            to_bytes(body, usize::MAX).await.unwrap().as_ref(),
            data.as_slice()
        );
        assert_eq!(*seen.lock().unwrap(), [Outcome::Complete]);
    }

    #[tokio::test]
    async fn a_body_dropped_part_way_says_how_far_it_got_and_only_once() {
        let (seen, done) = recorder();
        let mut stream = Tracked {
            data: Bytes::from(vec![0; CHUNK * 4]),
            sent: 0,
            started: Instant::now(),
            done: Some(done),
        };
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
        drop(stream);
        assert_eq!(
            *seen.lock().unwrap(),
            [Outcome::Aborted { sent: CHUNK * 2 }]
        );
    }

    #[tokio::test]
    async fn a_body_dropped_after_its_last_chunk_is_complete() {
        let (seen, done) = recorder();
        let mut stream = Tracked {
            data: Bytes::from(vec![0; CHUNK + 1]),
            sent: 0,
            started: Instant::now(),
            done: Some(done),
        };
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
        drop(stream);
        assert_eq!(*seen.lock().unwrap(), [Outcome::Complete]);
    }

    #[tokio::test]
    async fn an_empty_body_is_complete() {
        let (seen, done) = recorder();
        assert!(
            to_bytes(tracked(Vec::new(), done), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(*seen.lock().unwrap(), [Outcome::Complete]);
    }
}
