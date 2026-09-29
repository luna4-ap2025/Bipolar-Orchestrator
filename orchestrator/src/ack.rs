//! The logic loop and the manual API calls all read from the same planet
//! receiver (and the same explorer receiver), so the next message isn't
//! always the ack you're waiting for. For example a leftover
//! `KillPlanetResult` got read as the next `AsteroidAck`. `recv_ack` skips
//! messages until it finds the right one.

use crate::error::OrchestratorError;
use crossbeam_channel::Receiver;
use std::fmt::Debug;
use std::time::{Duration, Instant};

/// Waits for a message that `matches` accepts (`Ok`), logging and skipping
/// the others (`Err` gives the message back). `timeout` is for the whole
/// wait, not per message.
///
/// # Errors
/// `ChannelError` on timeout or if the channel is disconnected.
pub(crate) fn recv_ack<M: Debug, T>(
    rx: &Receiver<M>,
    timeout: Duration,
    context: &str,
    mut matches: impl FnMut(M) -> Result<T, M>,
) -> Result<T, OrchestratorError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(OrchestratorError::ChannelError(format!(
                "Timeout waiting for {context}"
            )));
        }

        match rx.recv_timeout(remaining) {
            Ok(msg) => match matches(msg) {
                Ok(value) => return Ok(value),
                Err(stray) => {
                    log::debug!("Discarding stray message while waiting for {context}: {stray:?}");
                }
            },
            Err(_) => {
                return Err(OrchestratorError::ChannelError(format!(
                    "Timeout waiting for {context}"
                )));
            }
        }
    }
}
