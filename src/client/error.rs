use actix_web::{
    dev::HttpResponseBuilder,
    error::JsonPayloadError,
    http::StatusCode,
    web::HttpResponse,
    ResponseError,
};
use displaydoc::Display;
use serde_json::{Error as JsonError, json};
use std::{
    io::Error as IoError,
    str::Utf8Error,
    string::FromUtf8Error,
};

use crate::util::AddEventError;

//TODO: should we expose any variant fields in display impl?
#[derive(Debug, Display)]
pub enum Error {
    /// Forbidden access, e.g. joining a room without permission, failed login.
    Forbidden,
    /// The access token specified was not recognised.
    UnknownToken,
    /// No access token was specified for the request.
    MissingToken,
    /// Request contained valid JSON, but it was malformed in some way, e.g. missing required keys,
    /// invalid values for keys.
    BadJson(String),
    /// Request did not contain valid JSON.
    NotJson(String),
    /// No resource was found for this request.
    NotFound,
    /// Too many requests have been sent in a short period of time.
    LimitExceeded,
    /// A required URL parameter was missing from the request.
    MissingParam(String),
    /// A specified URL parameter has an invalid value.
    InvalidParam(String),
    /// The specified room version is not supported.
    UnsupportedRoomVersion,

    /// An encoded string in the URL was not valid UTF-8.
    UrlNotUtf8(Utf8Error),
    /// A database error occurred.
    DbError(pg::Error),
    /// An I/O error occurred.
    IoError(IoError),
    /// A password error occurred.
    PasswordError(argon2::Error),
    /// The requested feature is unimplemented.
    Unimplemented,
    /// An invalid event was sent to a room.
    AddEventError(AddEventError<pg::Error>),
    /// An unknown error occurred.
    Unknown(String),
}

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        use Error::*;
        match self {
            Forbidden | UnknownToken | MissingToken => StatusCode::FORBIDDEN,
            NotFound => StatusCode::NOT_FOUND,
            BadJson(_) | NotJson(_) | MissingParam(_) | InvalidParam(_) | UnsupportedRoomVersion
                | UrlNotUtf8(_) | PasswordError(_) | Unknown(_) => StatusCode::BAD_REQUEST,
            LimitExceeded => StatusCode::TOO_MANY_REQUESTS,
            DbError(_) | IoError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Unimplemented => StatusCode::NOT_IMPLEMENTED,
        }
    }
    fn error_response(&self) -> HttpResponse {
        use Error::*;
        let (errcode, error) = match self {
            Forbidden => {
                ("M_FORBIDDEN",
                "Forbidden access, e.g. joining a room without permission, failed login.".to_string())
            },
            UnknownToken => {
                ("M_UNKNOWN_TOKEN",
                "The access token specified was not recognised.".to_string())
            },
            MissingToken => {
                ("M_MISSING_TOKEN",
                "No access token was specified for the request.".to_string())
            },
            BadJson(error) => {
                ("M_BAD_JSON",
                error.clone())
            },
            NotJson(error) => {
                ("M_NOT_JSON",
                error.clone())
            },
            NotFound => {
                ("M_NOT_FOUND",
                "No resource was found for this request.".to_string())
            },
            LimitExceeded => {
                ("M_LIMIT_EXCEEDED",
                "Too many requests have been sent in a short period of time. Wait a while then try again.".to_string())
            },
            MissingParam(error) => {
                ("M_MISSING_PARAM",
                error.clone())
            },
            InvalidParam(error) => {
                ("M_INVALID_PARAM",
                error.clone())
            },
            UnsupportedRoomVersion => {
                ("M_UNSUPPORTED_ROOM_VERSION",
                "The specified room version is not supported.".to_string())
            },
            UrlNotUtf8(error) => {
                ("M_UNKNOWN",
                format!("Malformed UTF-8 in URL: {}", error))
            },
            DbError(error) => {
                ("M_UNKNOWN",
                format!("{}", error))
            },
            IoError(error) => {
                ("M_UNKNOWN",
                format!("{}", error))
            },
            PasswordError(error) => {
                ("M_UNKNOWN",
                format!("{}", error))
            }
            Unimplemented => {
                ("M_UNKNOWN",
                "Feature unimplemented".to_string())
            },
            AddEventError(error) => {
                ("M_UNKNOWN",
                format!("{}", error))
            },
            Unknown(e) => {
                ("M_UNKNOWN",
                e.clone())
            },
        };
        HttpResponseBuilder::new(self.status_code())
            .json(json!({
                "errcode": errcode,
                "error": error
            }))
    }
}

impl From<Utf8Error> for Error {
    fn from(e: Utf8Error) -> Self {
        Error::NotJson(format!("{}", e))
    }
}

impl From<FromUtf8Error> for Error {
    fn from(e: FromUtf8Error) -> Self {
        Error::NotJson(format!("{}", e))
    }
}

impl From<JsonPayloadError> for Error {
    fn from(e: JsonPayloadError) -> Self {
        if let JsonPayloadError::Deserialize(e) = e {
            e.into()
        } else {
            Error::Unknown(format!("{}", e))
        }
    }
}

impl From<JsonError> for Error {
    fn from(e: JsonError) -> Self {
        use serde_json::error::Category;
        match e.classify() {
            Category::Data => Error::BadJson(format!("{}", e)),
            _ => Error::NotJson(format!("{}", e)),
        }
    }
}

impl From<IoError> for Error {
    fn from(e: IoError) -> Self {
        Error::IoError(e)
    }
}

impl From<pg::Error> for Error {
    fn from(e: pg::Error) -> Self {
        Error::DbError(e)
    }
}

impl From<argon2::Error> for Error {
    fn from(e: argon2::Error) -> Self {
        Error::PasswordError(e)
    }
}

impl From<AddEventError<pg::Error>> for Error {
    fn from(e: AddEventError<pg::Error>) -> Self {
        match e {
            AddEventError::DbError(e) => Error::DbError(e),
            _ => Error::AddEventError(e),
        }
    }
}

impl From<std::convert::Infallible> for Error {
    fn from(_: std::convert::Infallible) -> Self {
        unreachable!()
    }
}
