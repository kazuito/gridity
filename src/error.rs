use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Runtime,
    Usage,
    Input,
    Dependency,
}

impl ErrorKind {
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Runtime => 1,
            ErrorKind::Usage => 2,
            ErrorKind::Input => 3,
            ErrorKind::Dependency => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::Runtime => "runtime",
            ErrorKind::Usage => "usage",
            ErrorKind::Input => "input",
            ErrorKind::Dependency => "dependency",
        }
    }
}

/// An error with a stable exit code and an optional hint telling the caller what to try next.
#[derive(Debug)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CliError {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

pub fn usage(message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::Usage, message)
}

pub fn input(message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::Input, message)
}

pub fn dependency(message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::Dependency, message)
}
