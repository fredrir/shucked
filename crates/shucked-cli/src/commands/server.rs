use anyhow::Result;

use crate::ExitStatus;

pub(crate) fn server() -> Result<ExitStatus> {
    shucked_server::run()?;
    Ok(ExitStatus::Success)
}
