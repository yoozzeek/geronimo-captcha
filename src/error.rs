use std::fmt;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, CaptchaError>;

#[derive(Error, Debug)]
pub enum CaptchaError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("decode image")]
    Decode(#[source] image::ImageError),
    #[error("encode image")]
    Encode(#[source] image::ImageError),
    #[error("registry error: {0}")]
    Registry(#[from] RegistryRejection),
    #[error("system clock is before the unix epoch")]
    Clock,
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegistryRejection {
    AlreadyVerified,
    NotRegistered,
    MaxAttemptsLimitExceeded,
    AtCapacity,
}

impl fmt::Display for RegistryRejection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::AlreadyVerified => "ALREADY_VERIFIED",
            Self::NotRegistered => "NOT_REGISTERED",
            Self::MaxAttemptsLimitExceeded => "MAX_ATTEMPTS_LIMIT_EXCEEDED",
            Self::AtCapacity => "AT_CAPACITY",
        })
    }
}

impl std::error::Error for RegistryRejection {}
