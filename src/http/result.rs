/// Transport and HTTP outcome for one completed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpState {
    Success,
    HttpError,
    Error,
    Timeout,
    ConnectionError,
}

impl HttpState {
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::HttpError => "http-error",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::ConnectionError => "connection-error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HttpState;

    #[test]
    fn ui_labels_are_stable_at_the_presentation_boundary() {
        let cases = [
            (HttpState::Success, "success"),
            (HttpState::HttpError, "http-error"),
            (HttpState::Error, "error"),
            (HttpState::Timeout, "timeout"),
            (HttpState::ConnectionError, "connection-error"),
        ];

        for (state, label) in cases {
            assert_eq!(state.as_label(), label);
        }
    }
}
