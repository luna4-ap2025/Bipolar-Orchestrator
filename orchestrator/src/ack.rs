//! # Ack matching helper
//!
//! `dispatch_to_planet` (the autonomous logic loop), `send_sunray`, and
//! `send_asteroid` (manual API calls, e.g. from the GUI) all wait on the same
//! shared `Receiver<PlanetToOrchestrator>`. A message meant for one call can
//! otherwise be misread as the ack another call is waiting for — for example
//! a `KillPlanetResult` left behind after a planet died was being misread as
//! the next tick's `AsteroidAck`, silently corrupting state. The same class of
//! bug hits the shared `Receiver<ExplorerToOrchestrator>`: `stop_logic` used to
//! fire `StopExplorerAI` without waiting for its ack, and a later
//! `start_logic`'s explorer-ack wait would then pick up that stray
//! `StopExplorerAIResult` and hard-error instead of ignoring it.
//!
//! [`recv_ack`] waits for a message that `matches` accepts, discarding
//! (and logging) anything else, until the overall `timeout` elapses. Generic
//! over the message type so it works for both the planet and explorer channels.

use crate::error::OrchestratorError;
use crossbeam_channel::Receiver;
use std::fmt::Debug;
use std::time::{Duration, Instant};

/// Waits up to `timeout` (total, not per-message) for a message accepted by
/// `matches`. Non-matching messages are discarded and logged rather than
/// being treated as the expected ack.
///
/// `matches` returns `Ok(value)` to accept the message and stop waiting, or
/// `Err(message)` to hand the (unmatched) message back so it can be logged.
///
/// # Errors
/// Returns [`OrchestratorError::ChannelError`] if no matching message arrives
/// before the deadline, or if the channel disconnects.
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
