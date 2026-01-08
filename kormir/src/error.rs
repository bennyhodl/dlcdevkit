use std::fmt::{Display, Formatter};

/// Kormir error type
#[derive(Debug, Clone)]
pub enum Error {
    /// Invalid argument given
    #[deprecated(since = "1.0.9")]
    InvalidArgument,
    /// Invalid event ID given
    InvalidEventId,
    /// Invalid outcomes given
    InvalidOutcomes,
    /// Invalid base given
    InvalidBase,
    /// Invalid number of digits given
    InvalidNumberOfDigits,
    /// Invalid nonces given
    InvalidNonces,
    /// Attempted to sign an event that was already signed
    EventAlreadySigned,
    /// Event data was not found
    NotFound,
    /// The storage failed to read/save the data
    StorageFailure,
    /// User gave an invalid outcome
    InvalidOutcome,
    /// User gave an invalid event descriptor
    InvalidEventDescriptor,
    /// User gave an invalid announcement
    InvalidAnnouncement,
    /// An error that should never happen, if it does it's a bug
    Internal,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            #[allow(deprecated)]
            Error::InvalidArgument => write!(f, "Invalid argument given"),
            Error::InvalidEventId => write!(f, "Invalid event ID given"),
            Error::InvalidOutcomes => write!(f, "Invalid outcomes given"),
            Error::InvalidBase => write!(f, "Invalid base given"),
            Error::InvalidNumberOfDigits => write!(f, "Invalid number of digits given"),
            Error::InvalidNonces => write!(f, "Invalid nonces given"),
            Error::EventAlreadySigned => write!(f, "Event already signed"),
            Error::NotFound => write!(f, "Event data not found"),
            Error::StorageFailure => write!(f, "Storage failure"),
            Error::InvalidOutcome => write!(f, "Invalid outcome"),
            Error::InvalidEventDescriptor => write!(f, "Invalid event descriptor"),
            Error::InvalidAnnouncement => write!(f, "Invalid announcement"),
            Error::Internal => write!(f, "Internal error"),
        }
    }
}

impl std::error::Error for Error {}
