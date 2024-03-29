use displaydoc::Display;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

lazy_static! {
    static ref SERVER_NAME_REGEX: Regex =
        Regex::new(include_str!("./mxid_server_name.regex")).unwrap();
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash, PartialEq, Eq)]
#[serde(try_from = "String")]
pub struct MatrixId(String);

#[derive(Debug, Display)]
pub enum MxidError {
    /// A Matrix ID can only be 255 characters long, including the '@', localpart, ':' and domain.
    TooLong,
    /// A Matrix ID can only have lowercase letters, numbers, and `-_.=/`.
    InvalidChar,
    /// A Matrix ID must begin with an '@'.
    NoLeadingAt,
    /// A Matrix ID must contain exactly one colon.
    WrongNumberOfColons,
    /// A Matrix ID must contain a valid domain name.
    InvalidDomain,
}

impl MatrixId {
    pub fn new(localpart: &str, domain: &str) -> Result<Self, MxidError> {
        Self::validate_parts(localpart, domain)?;
        Ok(MatrixId(format!("@{}:{}", localpart, domain)))
    }

    pub fn as_str(&self) -> &str {
        &*self.0
    }

    pub fn to_string(self) -> String {
        self.0
    }

    pub fn clone_inner(&self) -> String {
        self.0.clone()
    }

    pub fn localpart(&self) -> &str {
        self.0.trim_start_matches('@').split(':').next().unwrap()
    }

    pub fn domain(&self) -> &str {
        self.0.split(':').nth(1).unwrap()
    }

    /// Verifies that a localpart and domain could together form a valid Matrix ID.
    pub fn validate_parts(localpart: &str, domain: &str) -> Result<(), MxidError> {
        if localpart.contains(|c: char| {
            !c.is_ascii_lowercase()
                && !c.is_ascii_digit()
                && c != '-'
                && c != '_'
                && c != '.'
                && c != '='
                && c != '/'
        }) {
            return Err(MxidError::InvalidChar);
        }

        if !SERVER_NAME_REGEX.is_match(domain) {
            return Err(MxidError::InvalidDomain);
        }

        if localpart.len() + domain.len() + 2 > 255 {
            return Err(MxidError::TooLong);
        }

        Ok(())
    }

    /// Verifies that a `&str` forms a valid Matrix ID.
    pub fn validate_all(mxid: &str) -> Result<(), MxidError> {
        if !mxid.starts_with('@') {
            return Err(MxidError::NoLeadingAt);
        }
        let remaining: &str = &mxid[1..];
        let (localpart, domain) = {
            let mut iter = remaining.split(':');
            let localpart = iter.next().unwrap();
            let domain = iter.next().ok_or(MxidError::WrongNumberOfColons)?;
            if iter.next() != None {
                return Err(MxidError::WrongNumberOfColons);
            }
            (localpart, domain)
        };
        Self::validate_parts(localpart, domain)?;

        Ok(())
    }
}

impl TryFrom<String> for MatrixId {
    type Error = MxidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        MatrixId::validate_all(&value)?;
        Ok(MatrixId(value))
    }
}

impl TryFrom<&str> for MatrixId {
    type Error = MxidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        MatrixId::validate_all(value)?;
        Ok(MatrixId(value.to_string()))
    }
}
