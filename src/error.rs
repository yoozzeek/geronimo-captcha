use crate::registry::RegistryCheckResult;

use std::error::Error as StdError;
use std::fmt;

pub type Result<T> = std::result::Result<T, CaptchaError>;

#[derive(Debug)]
pub enum CaptchaError {
    InvalidInput(&'static str),
    DecodeError(String),
    EncodeError(String),
    Registry(RegistryCheckResult),
    Internal(String),
}

impl fmt::Display for CaptchaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptchaError::InvalidInput(m) => write!(f, "invalid input: {m}"),
            CaptchaError::DecodeError(m) => write!(f, "decode error: {m}"),
            CaptchaError::EncodeError(m) => write!(f, "encode error: {m}"),
            CaptchaError::Registry(r) => write!(f, "registry error: {r}"),
            CaptchaError::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl StdError for CaptchaError {}
