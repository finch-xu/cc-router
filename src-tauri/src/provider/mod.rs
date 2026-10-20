pub mod loader;
pub mod model;
pub mod schema;
pub mod url_template;

pub use model::{Provider, ProviderEndpoint};

/// Filter suggestions only; saved slots and manually entered IDs remain valid.
/// Gateway IDs are retained only when supplied by that gateway, never synthesized.
pub fn is_current_text_suggestion(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let name = name.split(':').next().unwrap_or(name);
    let m_series = name.strip_prefix("minimax-m")
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|c| c.is_ascii_digit());
    if m_series || matches!(name, "minimax-01" | "minimax-latest") {
        matches!(name, "minimax-m3.1-flash" | "minimax-m3.1-flash-free" | "minimax-m3")
    } else {
        true
    }
}
