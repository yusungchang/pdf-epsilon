//! Text for people, without the text.
//!
//! The engine never writes sentences. It says which message it means, as a key into the
//! language files in `ui/locales`, and gives the values the message needs. The interface
//! picks the language, fills in the values and formats numbers, sizes and lists its own way.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    /// For example `finding.oversizedImages.title`.
    pub key: &'static str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<&'static str, Arg>,
}

/// A value inside a message. A `count` argument also picks the plural form.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Arg {
    Number(f64),
    Text(String),
    /// Joined the way the language joins lists.
    List(Vec<String>),
    /// A file size, formatted by the interface.
    Bytes { bytes: u64 },
    /// Another message key, translated in place: an image's colour space, a preset's name.
    Term { term: String },
}

impl Message {
    pub fn new(key: &'static str, args: Vec<(&'static str, Arg)>) -> Self {
        Message { key, args: args.into_iter().collect() }
    }
}

/// A file size, as a message argument.
pub struct Bytes(pub u64);

/// A key translated in place, as a message argument.
pub struct Term(pub String);

impl From<usize> for Arg {
    fn from(n: usize) -> Self {
        Arg::Number(n as f64)
    }
}
impl From<u32> for Arg {
    fn from(n: u32) -> Self {
        Arg::Number(n as f64)
    }
}
impl From<u64> for Arg {
    fn from(n: u64) -> Self {
        Arg::Number(n as f64)
    }
}
impl From<f32> for Arg {
    fn from(n: f32) -> Self {
        Arg::Number(n as f64)
    }
}
impl From<String> for Arg {
    fn from(s: String) -> Self {
        Arg::Text(s)
    }
}
impl From<&str> for Arg {
    fn from(s: &str) -> Self {
        Arg::Text(s.to_string())
    }
}
impl From<Vec<String>> for Arg {
    fn from(v: Vec<String>) -> Self {
        Arg::List(v)
    }
}
impl From<Bytes> for Arg {
    fn from(b: Bytes) -> Self {
        Arg::Bytes { bytes: b.0 }
    }
}
impl From<Term> for Arg {
    fn from(t: Term) -> Self {
        Arg::Term { term: t.0 }
    }
}

/// `msg!("finding.duplicates.title", count = 3)`
macro_rules! msg {
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::message::Message::new($key, vec![$((stringify!($name), $crate::message::Arg::from($value))),*])
    };
}
pub(crate) use msg;
