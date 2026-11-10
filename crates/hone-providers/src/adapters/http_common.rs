//! Helpers shared by the HTTP adapters (Anthropic, OpenAI-compatible).

/// Normalise a provider base URL: trim surrounding whitespace, drop trailing
/// `/`, then one trailing whole `/v1` segment, then trailing `/` again. Request
/// sites append `/v1/...` themselves, so `https://openrouter.ai/api/v1` must not
/// become `.../v1/v1/...`. `/apiv1`, `/v1beta` and `/v10` are left alone.
pub(crate) fn normalize_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let without_v1 = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    without_v1.trim_end_matches('/').to_owned()
}
