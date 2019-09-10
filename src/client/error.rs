use serde_json::{Error as JsonError, json};
use tide::{
    http::status::StatusCode,
    response::{self, IntoResponse, Response},
};

use std::{
    io::Error as IoError,
    str::Utf8Error,
};

#[derive(Debug)]
pub enum Error {
    Forbidden,
    UnknownToken,
    MissingToken,
    BadJson(String),
    NotJson(String),
    NotFound,
    LimitExceeded,
    MissingParam(String),
    InvalidParam(String),

    UrlNotUtf8(Utf8Error),
    DbError(pg::Error),
    IoError(IoError),
    PasswordError(argon2::Error),
    Unimplemented,
    Unknown(String),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        tracing::debug!("{:?}", &self);
        use Error::*;
        let (errcode, error, http_code) = match self {
            Forbidden => {
                ("M_FORBIDDEN",
                "Forbidden access, e.g. joining a room without permission, failed login.".to_string(),
                StatusCode::FORBIDDEN)
            },
            UnknownToken => {
                ("M_UNKNOWN_TOKEN",
                "The access token specified was not recognised.".to_string(),
                StatusCode::FORBIDDEN)
            },
            MissingToken => {
                ("M_MISSING_TOKEN",
                "No access token was specified for the request.".to_string(),
                StatusCode::FORBIDDEN)
            },
            BadJson(error) => {
                ("M_BAD_JSON",
                format!("{}", error),
                StatusCode::BAD_REQUEST)
            },
            NotJson(error) => {
                ("M_NOT_JSON",
                format!("{}", error),
                StatusCode::BAD_REQUEST)
            },
            NotFound => {
                ("M_NOT_FOUND",
                "No resource was found for this request.".to_string(),
                StatusCode::NOT_FOUND)
            },
            LimitExceeded => {
                ("M_LIMIT_EXCEEDED",
                "Too many requests have been sent in a short period of time. Wait a while then try again.".to_string(),
                StatusCode::TOO_MANY_REQUESTS)
            },
            MissingParam(error) => {
                ("M_MISSING_PARAM",
                format!("Missing URL parameter: {}", error),
                StatusCode::BAD_REQUEST)
            },
            InvalidParam(error) => {
                ("M_INVALID_PARAM",
                format!("Invalid URL parameter: {}", error),
                StatusCode::BAD_REQUEST)
            },
            UrlNotUtf8(error) => {
                ("M_UNKNOWN",
                format!("Malformed UTF-8 in URL: {}", error),
                StatusCode::BAD_REQUEST)
            },
            DbError(error) => {
                ("M_UNKNOWN",
                format!("{}", error),
                StatusCode::INTERNAL_SERVER_ERROR)
            },
            IoError(error) => {
                ("M_UNKNOWN",
                format!("{}", error),
                StatusCode::INTERNAL_SERVER_ERROR)
            },
            PasswordError(error) => {
                ("M_UNKNOWN",
                format!("{}", error),
                StatusCode::BAD_REQUEST)
            }
            Unimplemented => {
                ("M_UNKNOWN",
                "Feature unimplemented".into(),
                StatusCode::NOT_IMPLEMENTED)
            },
            Unknown(e) => {
                ("M_UNKNOWN",
                e,
                StatusCode::BAD_REQUEST)
            }
        };
        response::json(json!({
            "errcode": errcode,
            "error": error
        })).with_status(http_code).into_response()
    }
}

impl From<Utf8Error> for Error {
    fn from(e: Utf8Error) -> Self {
        Error::NotJson(format!("{}", e))
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

impl From<std::convert::Infallible> for Error {
    fn from(e: std::convert::Infallible) -> Self {
        unreachable!()
    }
}
