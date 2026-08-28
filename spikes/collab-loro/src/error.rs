use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    Empty {
        input: &'static str,
    },
    LimitExceeded {
        input: &'static str,
        actual_bytes: usize,
        max_bytes: usize,
    },
}

impl Display for InputError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty { input } => write!(formatter, "{input} must not be empty"),
            Self::LimitExceeded {
                input,
                actual_bytes,
                max_bytes,
            } => write!(
                formatter,
                "{input} is {actual_bytes} bytes, exceeding the {max_bytes}-byte limit"
            ),
        }
    }
}

impl Error for InputError {}
