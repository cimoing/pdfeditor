use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug)]
pub enum CoreError {
    Io(io::Error),
    InvalidPdf(String),
    Unsupported(String),
    NotFound(String),
    InvalidOperation(String),
    Engine(String),
}

impl Display for CoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Io(err) => write!(f, "I/O error: {err}"),
            CoreError::InvalidPdf(message) => write!(f, "invalid PDF: {message}"),
            CoreError::Unsupported(message) => write!(f, "unsupported operation: {message}"),
            CoreError::NotFound(message) => write!(f, "not found: {message}"),
            CoreError::InvalidOperation(message) => write!(f, "invalid operation: {message}"),
            CoreError::Engine(message) => write!(f, "PDF engine error: {message}"),
        }
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            CoreError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for CoreError {
    fn from(value: io::Error) -> Self {
        CoreError::Io(value)
    }
}
