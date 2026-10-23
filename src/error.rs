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

/// Why an error happened, printed as `reason` in JSON so callers can branch on it. The names
/// are part of the output contract (`docs/guide/output.md`): add new ones, never rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    InvalidArgument,
    InvalidValue,
    PageOutOfRange,
    FlagNotForInput,
    LabelVariableNotForInput,
    TooManySheets,
    TooManySamples,
    TooManyFiles,
    DoesNotFit,
    OutsideMedia,
    UnknownTopic,
    OutsideRoot,
    TooLarge,
    NotFound,
    UnknownType,
    WrongType,
    Unreadable,
    DecodeFailed,
    TooLargeToDecode,
    PasswordRequired,
    PasswordIncorrect,
    DecryptFailed,
    InvalidPdf,
    NoPages,
    NoImages,
    NoFrames,
    NoVideoStream,
    UnknownDuration,
    Timeout,
    ToolMissing,
    UnsafeOutputDir,
    Internal,
    Other,
}

impl Reason {
    pub const ALL: [Reason; 33] = [
        Reason::InvalidArgument,
        Reason::InvalidValue,
        Reason::PageOutOfRange,
        Reason::FlagNotForInput,
        Reason::LabelVariableNotForInput,
        Reason::TooManySheets,
        Reason::TooManySamples,
        Reason::TooManyFiles,
        Reason::DoesNotFit,
        Reason::OutsideMedia,
        Reason::UnknownTopic,
        Reason::OutsideRoot,
        Reason::TooLarge,
        Reason::NotFound,
        Reason::UnknownType,
        Reason::WrongType,
        Reason::Unreadable,
        Reason::DecodeFailed,
        Reason::TooLargeToDecode,
        Reason::PasswordRequired,
        Reason::PasswordIncorrect,
        Reason::DecryptFailed,
        Reason::InvalidPdf,
        Reason::NoPages,
        Reason::NoImages,
        Reason::NoFrames,
        Reason::NoVideoStream,
        Reason::UnknownDuration,
        Reason::Timeout,
        Reason::ToolMissing,
        Reason::UnsafeOutputDir,
        Reason::Internal,
        Reason::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Reason::InvalidArgument => "invalid_argument",
            Reason::InvalidValue => "invalid_value",
            Reason::PageOutOfRange => "page_out_of_range",
            Reason::FlagNotForInput => "flag_not_for_input",
            Reason::LabelVariableNotForInput => "label_variable_not_for_input",
            Reason::TooManySheets => "too_many_sheets",
            Reason::TooManySamples => "too_many_samples",
            Reason::TooManyFiles => "too_many_files",
            Reason::DoesNotFit => "does_not_fit",
            Reason::OutsideMedia => "outside_media",
            Reason::UnknownTopic => "unknown_topic",
            Reason::OutsideRoot => "outside_root",
            Reason::TooLarge => "too_large",
            Reason::NotFound => "not_found",
            Reason::UnknownType => "unknown_type",
            Reason::WrongType => "wrong_type",
            Reason::Unreadable => "unreadable",
            Reason::DecodeFailed => "decode_failed",
            Reason::TooLargeToDecode => "too_large_to_decode",
            Reason::PasswordRequired => "password_required",
            Reason::PasswordIncorrect => "password_incorrect",
            Reason::DecryptFailed => "decrypt_failed",
            Reason::InvalidPdf => "invalid_pdf",
            Reason::NoPages => "no_pages",
            Reason::NoImages => "no_images",
            Reason::NoFrames => "no_frames",
            Reason::NoVideoStream => "no_video_stream",
            Reason::UnknownDuration => "unknown_duration",
            Reason::Timeout => "timeout",
            Reason::ToolMissing => "tool_missing",
            Reason::UnsafeOutputDir => "unsafe_output_dir",
            Reason::Internal => "internal",
            Reason::Other => "other",
        }
    }
}

/// An error with a stable exit code, a reason and an optional hint telling the caller what to
/// try next.
#[derive(Debug)]
pub struct CliError {
    pub kind: ErrorKind,
    pub reason: Reason,
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn new(kind: ErrorKind, reason: Reason, message: impl Into<String>) -> Self {
        CliError {
            kind,
            reason,
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

pub fn usage(reason: Reason, message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::Usage, reason, message)
}

pub fn input(reason: Reason, message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::Input, reason, message)
}

pub fn dependency(reason: Reason, message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::Dependency, reason, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_is_documented_once() {
        let guide = include_str!("../docs/guide/output.md");
        let mut names: Vec<&str> = Reason::ALL.iter().map(|r| r.name()).collect();
        for name in &names {
            assert!(
                guide.contains(&format!("`{name}`")),
                "reason {name} is missing from docs/guide/output.md"
            );
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Reason::ALL.len());
    }
}
