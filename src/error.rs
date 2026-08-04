use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    AlreadyExists,
    PermissionDenied,
    Unsupported,
    Unavailable,
    AuthenticationFailed,
    HostKeyVerificationFailed,
    Network,
    Timeout,
    Interrupted,
    NoSpace,
    Conflict,
    InvalidInput,
    InvalidConfiguration,
    ExternalTool,
    Integrity,
    Internal,
}

#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct ArxError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ArxError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, ArxError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kind_is_structured() {
        let error = ArxError::new(ErrorKind::NoSpace, "destination is full");

        assert_eq!(error.kind, ErrorKind::NoSpace);
    }
}
