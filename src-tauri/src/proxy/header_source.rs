//! Where an outbound header came from, recorded when the header map is
//! assembled (request log + header preview). The frontend renders it as-is.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderSource {
    /// Client header forwarded by the header policy or yaml `forward_headers`.
    Forwarded,
    /// Fixed by the provider: yaml `required_headers` / endpoint headers, or
    /// written by a translation dispatch (Codex UA, originator, ...).
    Provider,
    /// Configured by the user: `extra_headers`, or `required_headers` of a
    /// user-defined subscription.
    Custom,
    /// The subscription's auth header.
    Auth,
    /// Added by cc-router itself: content-type, accept, default UA/Accept-Encoding.
    CcRouter,
}

impl HeaderSource {
    /// Same string as the serde name; used in the stored log JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forwarded => "forwarded",
            Self::Provider => "provider",
            Self::Custom => "custom",
            Self::Auth => "auth",
            Self::CcRouter => "cc_router",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_serde_name() {
        for s in [
            HeaderSource::Forwarded,
            HeaderSource::Provider,
            HeaderSource::Custom,
            HeaderSource::Auth,
            HeaderSource::CcRouter,
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{}\"", s.as_str()));
        }
    }
}
