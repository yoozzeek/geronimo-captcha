use crate::error::{CaptchaError, Result};

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const MAX_TTL_SECS: u64 = 86_400;

pub fn get_timestamp() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| CaptchaError::Clock)
}
