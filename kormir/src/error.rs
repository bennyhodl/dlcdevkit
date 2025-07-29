use std::fmt::{Display, Formatter};

/// Kormir error type
#[derive(Debug, Clone)]
pub enum Error {
    /// Invalid argument given
    InvalidArgument,
    /// Attempted to sign an event that was already signed
    EventAlreadySigned,
    /// Event data was not found
    NotFound,
    /// The storage failed to read/save the data
    StorageFailure,
    /// User gave an invalid outcome
    InvalidOutcome,
    /// An error that should never happen, if it does it's a bug
    Internal,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidArgument => write!(f, "Invalid argument given"),
            Error::EventAlreadySigned => write!(f, "Event already signed"),
            Error::NotFound => write!(f, "Event data not found"),
            Error::StorageFailure => write!(f, "Storage failure"),
            Error::InvalidOutcome => write!(f, "Invalid outcome"),
            Error::Internal => write!(f, "Internal error"),
        }
    }
}

impl std::error::Error for Error {}
