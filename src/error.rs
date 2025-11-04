use crate::registry::RegistryCheckResult;
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
    Registry(RegistryCheckResult),
    #[error("internal error: {0}")]
    Internal(String),
}
