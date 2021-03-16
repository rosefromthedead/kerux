use std::{fmt::Display, io::Error as IoError, str::Utf8Error, string::FromUtf8Error};

use actix_web::{HttpResponse, ResponseError, dev::HttpResponseBuilder, error::JsonPayloadError, http::StatusCode};
use displaydoc::Display;
use serde_json::{Error as JsonError, json};
use tracing_error::SpanTrace;

use crate::util::storage::AddEventError;

// All-seeing all-knowing error type
#[derive(Debug)]
pub struct Error {
    inner: ErrorKind,
    spantrace: SpanTrace,
}

impl<T: Into<ErrorKind>> From<T> for Error {
    fn from(inner: T) -> Self {
        let spantrace = tracing_error::SpanTrace::capture();
        Error { inner: inner.into(), spantrace }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n{}", self.inner, self.spantrace)
    }
}

#[derive(Debug, Display)]
pub enum ErrorKind {
    /// Forbidden access, e.g. joining a room without permission, failed login.
    Forbidden,
    /// The access token specified was not recognised.
    UnknownToken,
    /// No access token was specified for the request.
    MissingToken,
    /// Request contained valid JSON, but it was malformed in some way, e.g. missing required keys,
    /// invalid values for keys: {0}
    BadJson(String),
    /// Request did not contain valid JSON: {0}
    NotJson(String),
    /// No resource was found for this request.
    NotFound,
    /// The specified user was not found on this server.
    UserNotFound(String),
    /// The specified room was not found on this server.
    RoomNotFound(String),
    /// Too many requests have been sent in a short period of time.
    LimitExceeded,
    /// A required URL parameter was missing from the request: {0}
    MissingParam(String),
    /// A specified URL parameter has an invalid value: {0}
    InvalidParam(String),
    /// The specified room version is not supported.
    UnsupportedRoomVersion,
    /// The specified transaction has already been started.
    TxnIdExists,

    /// An encoded string in the URL was not valid UTF-8: {0}
    UrlNotUtf8(Utf8Error),
    /// A database error occurred.
    DbError,
    /// A password error occurred: {0}
    PasswordError(argon2::Error),
    /// The requested feature is unimplemented.
    Unimplemented,
    /// An invalid event was sent to a room: {0}
    AddEventError(AddEventError),
    /// An unknown error occurred: {0}
    Unknown(String),
}

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        use ErrorKind::*;
        match self.inner {
            Forbidden | UnknownToken | MissingToken => StatusCode::FORBIDDEN,
            NotFound | UserNotFound(_) | RoomNotFound(_) => StatusCode::NOT_FOUND,
            BadJson(_) | NotJson(_) | MissingParam(_) | InvalidParam(_) | UnsupportedRoomVersion
                | UrlNotUtf8(_) | PasswordError(_) | Unknown(_)
                | TxnIdExists => StatusCode::BAD_REQUEST,
            LimitExceeded => StatusCode::TOO_MANY_REQUESTS,
            DbError | AddEventError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Unimplemented => StatusCode::NOT_IMPLEMENTED,
        }
    }
    fn error_response(&self) -> HttpResponse {
        use ErrorKind::*;
        let errcode = match self.inner {
            Forbidden => "M_FORBIDDEN",
            UnknownToken => "M_UNKNOWN_TOKEN",
            MissingToken => "M_MISSING_TOKEN",
            BadJson(_) => "M_BAD_JSON",
            NotJson(_) => "M_NOT_JSON",
            NotFound | UserNotFound(_) | RoomNotFound(_) => "M_NOT_FOUND",
            LimitExceeded => "M_LIMIT_EXCEEDED",
            MissingParam(_) => "M_MISSING_PARAM",
            InvalidParam(_) => "M_INVALID_PARAM",
            UnsupportedRoomVersion => "M_UNSUPPORTED_ROOM_VERSION",
            TxnIdExists | UrlNotUtf8(_) | DbError | PasswordError(_)
                | Unimplemented | AddEventError(_) | Unknown(_) => "M_UNKNOWN",
        };
        let error = format!("{}", self);
        HttpResponseBuilder::new(self.status_code())
            .json(json!({
                "errcode": errcode,
                "error": error
            }))
    }
}

impl std::error::Error for Error {}

impl From<Utf8Error> for ErrorKind {
    fn from(e: Utf8Error) -> Self {
        ErrorKind::NotJson(format!("{}", e))
    }
}

impl From<FromUtf8Error> for ErrorKind {
    fn from(e: FromUtf8Error) -> Self {
        ErrorKind::NotJson(format!("{}", e))
    }
}

impl From<JsonPayloadError> for ErrorKind {
    fn from(e: JsonPayloadError) -> Self {
        if let JsonPayloadError::Deserialize(e) = e {
            e.into()
        } else {
            ErrorKind::Unknown(format!("{}", e))
        }
    }
}

impl From<JsonError> for ErrorKind {
    fn from(e: JsonError) -> Self {
        use serde_json::error::Category;
        match e.classify() {
            Category::Data => ErrorKind::BadJson(format!("{}", e)),
            _ => ErrorKind::NotJson(format!("{}", e)),
        }
    }
}

impl From<argon2::Error> for ErrorKind {
    fn from(e: argon2::Error) -> Self {
        ErrorKind::PasswordError(e)
    }
}

impl From<AddEventError> for ErrorKind {
    fn from(e: AddEventError) -> Self {
        ErrorKind::AddEventError(e)
    }
}
