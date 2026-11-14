use std::fmt;
use std::io;

/// Recoverable CLI failure. Broken pipes are reported separately so callers can
/// exit successfully.
#[derive(Debug)]
pub enum Error {
    Usage(String),
    Runtime(String),
    BrokenPipe,
}

impl Error {
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub(crate) fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(message.into())
    }

    pub(crate) fn code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            Self::Runtime(_) => 1,
            Self::BrokenPipe => 0,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Runtime(message) => formatter.write_str(message),
            Self::BrokenPipe => formatter.write_str("broken pipe"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::BrokenPipe {
            Self::BrokenPipe
        } else {
            Self::Runtime(err.to_string())
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
