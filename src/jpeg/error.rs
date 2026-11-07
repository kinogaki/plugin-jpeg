//! Why a JPEG could not be decoded or encoded.
use std::fmt;

/// Why a JPEG could not be decoded or encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file breaks the format; `at` is the byte offset where it was
    /// found.
    Corrupt {
        /// What is wrong.
        what: &'static str,
        /// Byte offset into the file.
        at: usize,
    },
    /// A valid JPEG this decoder does not read (lossless, arithmetic
    /// coding, 12-bit samples...).
    Unsupported(&'static str),
    /// The request itself is wrong (scale, quality, image shape).
    Invalid(&'static str),
    /// The read's budget refused what the file asks: why.
    Budget(String),
    /// The decode was cancelled.
    Cancelled,
}

impl Error {
    pub(crate) fn corrupt(what: &'static str, at: usize) -> Self {
        Error::Corrupt { what, at }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt { what, at } => write!(f, "invalid JPEG at byte {at}: {what}"),
            Error::Unsupported(what) => write!(f, "unsupported JPEG: {what}"),
            Error::Invalid(what) => write!(f, "invalid JPEG request: {what}"),
            Error::Budget(why) => write!(f, "JPEG image: {why}"),
            Error::Cancelled => write!(f, "JPEG decoding cancelled"),
        }
    }
}

/// What the read's budget refused, or its cancellation.
impl From<kinogaki_studio_api::Error> for Error {
    fn from(e: kinogaki_studio_api::Error) -> Self {
        match e.is_cancelled() {
            true => Error::Cancelled,
            false => Error::Budget(e.message().to_owned()),
        }
    }
}

impl std::error::Error for Error {}
