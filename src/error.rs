use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalInferenceError {
    code: &'static str,
}

impl LocalInferenceError {
    pub(crate) const fn new(code: &'static str) -> Self {
        Self { code }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for LocalInferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for LocalInferenceError {}
