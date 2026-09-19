use crate::session::RequestCancellationToken;
use std::process::Command;
use std::time::Duration;

pub(crate) fn capture(
    command: &mut Command,
    timeout: Duration,
    cancellation: &RequestCancellationToken,
    zpty: bool,
) -> Option<Vec<u8>> {
    shucked_command::process::capture(command, timeout, &|| cancellation.is_cancelled(), zpty)
}
