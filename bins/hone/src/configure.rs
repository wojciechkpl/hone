//! Interactive configuration wizard for `hone configure`.
//!
//! Guides the user through provider selection, API key entry, and model
//! selection, then writes `~/.config/hone/config.toml`.
//!
//! No extra dependencies — input is read via `std::io::stdin().read_line()`.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

// ── Provider catalogue ────────────────────────────────────────────────────────

struct ProviderEntry {
    name: &'static str,
    display: &'static str,
    models: &'static [&'static str],
    /// Hint shown next to the API key prompt.
    key_hint: &'static str,
    /// Optional fixed base URL (used for OpenRouter / DeepSeek).
    base_url: Option<&'static str>,
    /// Whether this provider requires an API key.
    needs_key: bool,
}

const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        name: "anthropic",
        display: "Anthropic (Claude)",
        models: &[
            "claude-sonnet-4-5",
            "claude-opus-4-5",
            "claude-haiku-4-5",
            "claude-3-7-sonnet-latest",
        ],
        key_hint: "sk-ant-...",
        base_url: None,
        needs_key: true,
    },
    ProviderEntry {
        name: "openai",
        display: "OpenAI",
        models: &["gpt-4o", "gpt-4o-mini", "gpt-4-turbo", "o3-mini"],
        key_hint: "sk-...",
        base_url: None,
        needs_key: true,
    },
    ProviderEntry {
        name: "deepseek",
        display: "DeepSeek",
        models: &["deepseek-chat", "deepseek-coder", "deepseek-reasoner"],
        key_hint: "sk-...",
        base_url: Some("https://api.deepseek.com"),
        needs_key: true,
    },
    ProviderEntry {
        name: "openrouter",
        display: "OpenRouter",
        models: &[
            "anthropic/claude-sonnet-4-5",
            "openai/gpt-4o",
            "google/gemini-2.5-pro",
            "meta-llama/llama-3.3-70b-instruct",
        ],
        key_hint: "sk-or-...",
        base_url: Some("https://openrouter.ai/api/v1"),
        needs_key: true,
    },
    ProviderEntry {
        name: "ollama",
        display: "Ollama (local)",
        models: &["llama3.2", "llama3.1", "qwen2.5-coder", "mistral"],
        key_hint: "(no key needed)",
        base_url: Some("http://localhost:11434"),
        needs_key: false,
    },
    ProviderEntry {
        name: "claude-cli",
        display: "Claude CLI (claude binary)",
        models: &["claude-sonnet-4-5", "claude-opus-4-5"],
        key_hint: "(no key needed — uses your Claude CLI login)",
        base_url: None,
        needs_key: false,
    },
];

// ── Wizard entry point ────────────────────────────────────────────────────────

/// Run the interactive configure wizard.  Writes config to
/// `~/.config/hone/config.toml` on success.
pub fn run_wizard() -> Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();

    // Step 1: provider selection
    println!("\nWelcome to the Hone configuration wizard.\n");
    println!("Step 1: Select a provider\n");
    for (i, p) in PROVIDERS.iter().enumerate() {
        println!("  {}. {}", i + 1, p.display);
    }
    println!();

    let provider = loop {
        print!("Enter number [1-{}]: ", PROVIDERS.len());
        io::stdout().flush()?;
        let choice = read_line(&mut input)?;
        match choice.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= PROVIDERS.len() => break &PROVIDERS[n - 1],
            _ => eprintln!("  Please enter a number between 1 and {}.", PROVIDERS.len()),
        }
    };

    println!("\nSelected: {}\n", provider.display);

    // Step 2: API key (skip when not needed)
    let api_key = if provider.needs_key {
        println!("Step 2: Enter your API key");
        println!("  (Input is not masked — paste your key and press Enter)");
        println!("  Format: {}\n", provider.key_hint);
        print!("API key: ");
        io::stdout().flush()?;
        let key = read_line(&mut input)?;
        let key = key.trim().to_owned();
        if key.is_empty() {
            anyhow::bail!("API key cannot be empty for {}", provider.display);
        }
        println!("  Key recorded.\n");
        Some(key)
    } else {
        println!("Step 2: API key\n  {} — skipping.\n", provider.key_hint);
        None
    };

    // Step 3: model selection
    println!("Step 3: Select a model\n");
    for (i, m) in provider.models.iter().enumerate() {
        println!("  {}. {}", i + 1, m);
    }
    println!();

    let model = loop {
        print!("Enter number [1-{}] (or type a custom model name): ", provider.models.len());
        io::stdout().flush()?;
        let choice = read_line(&mut input)?;
        let choice = choice.trim();
        // Allow numeric selection or free-form model name.
        if let Ok(n) = choice.parse::<usize>() {
            if n >= 1 && n <= provider.models.len() {
                break provider.models[n - 1].to_owned();
            }
            eprintln!("  Enter a number between 1 and {} or a model name.", provider.models.len());
        } else if !choice.is_empty() {
            break choice.to_owned();
        } else {
            eprintln!("  Please make a selection.");
        }
    };

    println!("\nSelected model: {model}\n");

    // Step 4: write config
    let config_path = config_path()?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    }

    let toml = build_toml(provider, api_key.as_deref(), &model);
    std::fs::write(&config_path, &toml)
        .with_context(|| format!("failed to write config to {}", config_path.display()))?;

    // Step 5: success
    println!("Step 4: Configuration saved to {}\n", config_path.display());
    println!("Configuration saved! Try: hone 'hello world'\n");

    Ok(())
}

// ── TOML generation ───────────────────────────────────────────────────────────

fn build_toml(provider: &ProviderEntry, api_key: Option<&str>, model: &str) -> String {
    let mut out = String::new();

    out.push_str("[provider]\n");
    out.push_str(&format!("name = {:?}\n", provider.name));

    if let Some(key) = api_key {
        out.push_str(&format!("api_key = {:?}\n", key));
    }

    out.push_str(&format!("model = {:?}\n", model));

    if let Some(url) = provider.base_url {
        out.push('\n');
        out.push_str(&format!("base_url = {:?}\n", url));
    }

    out
}

// ── Config path ───────────────────────────────────────────────────────────────

fn config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(PathBuf::from(home).join(".config/hone/config.toml"))
}

/// Read the wizard config from `~/.config/hone/config.toml` and apply any
/// settings that are not already covered by environment variables.
///
/// Currently handles:
/// - `[provider].api_key`    → sets `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`
/// - `[provider].base_url`   → sets `HONE_OPENAI_BASE_URL`
/// - `[provider].model`      → sets `HONE_OPENAI_MODEL` (for openai-compat paths)
///
/// Env vars already set by the user always take priority.
pub fn apply_wizard_config() {
    let Ok(home) = std::env::var("HOME") else { return };
    let path = PathBuf::from(home).join(".config/hone/config.toml");
    let Ok(raw) = std::fs::read_to_string(&path) else { return };

    // Minimal TOML parsing — we only need a small, well-known subset.
    let cfg = match WizardConfig::parse(&raw) {
        Some(c) => c,
        None => return,
    };

    match cfg.provider_name.as_deref() {
        Some("anthropic") => {
            if let Some(key) = &cfg.api_key {
                if std::env::var("ANTHROPIC_API_KEY").is_err() {
                    // SAFETY: called before any other threads are spawned in main.
                    unsafe { std::env::set_var("ANTHROPIC_API_KEY", key) };
                }
            }
        }
        Some("openai" | "deepseek" | "openrouter") => {
            if let Some(key) = &cfg.api_key {
                if std::env::var("OPENAI_API_KEY").is_err() {
                    unsafe { std::env::set_var("OPENAI_API_KEY", key) };
                }
            }
            if let Some(url) = &cfg.base_url {
                if std::env::var("HONE_OPENAI_BASE_URL").is_err() {
                    unsafe { std::env::set_var("HONE_OPENAI_BASE_URL", url) };
                }
            }
            if let Some(model) = &cfg.model {
                if std::env::var("HONE_OPENAI_MODEL").is_err() {
                    unsafe { std::env::set_var("HONE_OPENAI_MODEL", model) };
                }
            }
        }
        _ => {} // claude-cli, ollama, unknown — no env vars needed
    }
}

// ── Minimal TOML reader for the wizard format ─────────────────────────────────

struct WizardConfig {
    provider_name: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
}

impl WizardConfig {
    /// Parse the wizard-format TOML.  Returns `None` on any parse error so
    /// that a malformed config never prevents startup.
    fn parse(raw: &str) -> Option<Self> {
        let mut in_provider = false;
        let mut provider_name = None;
        let mut api_key = None;
        let mut model = None;
        let mut base_url = None;

        for line in raw.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_provider = line == "[provider]";
                continue;
            }
            if !in_provider {
                continue;
            }
            // Parse `key = "value"` lines.
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = unquote(v.trim())?;
                match k {
                    "name" => provider_name = Some(v),
                    "api_key" => api_key = Some(v),
                    "model" => model = Some(v),
                    "base_url" => base_url = Some(v),
                    _ => {}
                }
            }
        }

        Some(WizardConfig { provider_name, api_key, model, base_url })
    }
}

/// Strip surrounding double-quotes from a TOML string value.
fn unquote(s: &str) -> Option<String> {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Some(s[1..s.len() - 1].to_owned())
    } else {
        None
    }
}

// ── I/O helper ────────────────────────────────────────────────────────────────

fn read_line(input: &mut impl BufRead) -> Result<String> {
    let mut buf = String::new();
    input.read_line(&mut buf).context("failed to read from stdin")?;
    Ok(buf)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_toml_anthropic() {
        let p = &PROVIDERS[0]; // anthropic
        let toml = build_toml(p, Some("sk-ant-test"), "claude-sonnet-4-5");
        assert!(toml.contains("[provider]"));
        assert!(toml.contains(r#"name = "anthropic""#));
        assert!(toml.contains(r#"api_key = "sk-ant-test""#));
        assert!(toml.contains(r#"model = "claude-sonnet-4-5""#));
        // anthropic has no base_url
        assert!(!toml.contains("base_url"));
    }

    #[test]
    fn build_toml_deepseek_includes_base_url() {
        let p = &PROVIDERS[2]; // deepseek
        let toml = build_toml(p, Some("sk-ds"), "deepseek-chat");
        assert!(toml.contains("base_url"));
        assert!(toml.contains("deepseek.com"));
    }

    #[test]
    fn build_toml_ollama_no_key() {
        let p = &PROVIDERS[4]; // ollama
        let toml = build_toml(p, None, "llama3.2");
        assert!(!toml.contains("api_key"));
        assert!(toml.contains("ollama"));
    }

    #[test]
    fn wizard_config_parse_anthropic() {
        let raw = r#"
[provider]
name = "anthropic"
api_key = "sk-ant-test"
model = "claude-sonnet-4-5"
"#;
        let cfg = WizardConfig::parse(raw).unwrap();
        assert_eq!(cfg.provider_name.as_deref(), Some("anthropic"));
        assert_eq!(cfg.api_key.as_deref(), Some("sk-ant-test"));
        assert_eq!(cfg.model.as_deref(), Some("claude-sonnet-4-5"));
        assert!(cfg.base_url.is_none());
    }

    #[test]
    fn wizard_config_parse_deepseek() {
        let raw = r#"
[provider]
name = "deepseek"
api_key = "sk-ds"
model = "deepseek-chat"
base_url = "https://api.deepseek.com"
"#;
        let cfg = WizardConfig::parse(raw).unwrap();
        assert_eq!(cfg.provider_name.as_deref(), Some("deepseek"));
        assert_eq!(cfg.base_url.as_deref(), Some("https://api.deepseek.com"));
    }

    #[test]
    fn unquote_strips_quotes() {
        assert_eq!(unquote(r#""hello""#), Some("hello".into()));
        assert_eq!(unquote(r#""""#), Some("".into()));
        assert_eq!(unquote("noquotes"), None);
        assert_eq!(unquote(r#""only_open"#), None);
    }
}
