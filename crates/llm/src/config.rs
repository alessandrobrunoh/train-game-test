//! Configuration from `.env` and the environment.
//!
//! | Variable | Meaning | Default |
//! |---|---|---|
//! | `LLM_API_URL` | Base URL of an OpenAI-compatible API (`…/v1`), or the full `…/chat/completions` endpoint. Unset: AI off. | — |
//! | `LLM_API_KEY` | Bearer key. Empty for local servers (Ollama, LM Studio). | none |
//! | `LLM_MODEL` | Model name, required when the URL is set. | — |
//! | `LLM_TIMEOUT_SECS` | Timeout of one call. | 60 |
//! | `LLM_MAX_CALLS_PER_HOUR` | Hourly budget (real time); 0 blocks every call. | 120 |
//! | `LLM_MAX_TOKENS` | Default length limit of an answer. | 800 |
//! | `LLM_TEMPERATURE` | Default temperature; unset: the provider's. | — |
//! | `LLM_JSON_MODE` | Send `response_format: json_object` for JSON requests (`true`/`false`). | false |
//! | `LLM_MAX_RETRIES` | Extra attempts on rate limits, overloads and network errors. | 1 |
//!
//! Variables already set in the process win over the `.env` file, which is
//! searched in the current directory and its parents. The process
//! environment is never modified.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, PartialEq)]
pub struct LlmConfig {
    /// Full URL of the chat completions endpoint.
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub max_calls_per_hour: u32,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub json_mode: bool,
    pub max_retries: u32,
}

/// Debug without the key: configs end up in logs.
impl fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("max_calls_per_hour", &self.max_calls_per_hour)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("json_mode", &self.json_mode)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A required variable is missing (the URL is set but not the model).
    Missing(&'static str),
    /// A variable has a value that can't be used.
    Invalid { var: &'static str, value: String },
    /// The `.env` file exists but can't be read.
    Unreadable { path: PathBuf, error: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Missing(var) => write!(f, "{var} is required when LLM_API_URL is set"),
            ConfigError::Invalid { var, value } => write!(f, "invalid value for {var}: {value:?}"),
            ConfigError::Unreadable { path, error } => {
                write!(f, "can't read {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl LlmConfig {
    /// The configuration from the environment and the nearest `.env`.
    /// `Ok(None)` when `LLM_API_URL` is unset: the AI is off.
    pub fn load() -> Result<Option<Self>, ConfigError> {
        let file = match find_dotenv() {
            Some(path) => {
                let text = std::fs::read_to_string(&path).map_err(|e| ConfigError::Unreadable {
                    path: path.clone(),
                    error: e.to_string(),
                })?;
                parse_dotenv(&text)
            }
            None => HashMap::new(),
        };
        Self::from_vars(|key| std::env::var(key).ok().or_else(|| file.get(key).cloned()))
    }

    /// The configuration from any source of variables (tests use a map).
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, ConfigError> {
        let get = |key: &str| {
            var(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(url) = get("LLM_API_URL") else {
            return Ok(None);
        };
        let model = get("LLM_MODEL").ok_or(ConfigError::Missing("LLM_MODEL"))?;
        fn number<T: std::str::FromStr>(
            var: &'static str,
            value: Option<String>,
            default: T,
        ) -> Result<T, ConfigError> {
            match value {
                None => Ok(default),
                Some(v) => v
                    .parse()
                    .map_err(|_| ConfigError::Invalid { var, value: v }),
            }
        }
        let timeout: f32 = number("LLM_TIMEOUT_SECS", get("LLM_TIMEOUT_SECS"), 60.0)?;
        if !(timeout > 0.0 && timeout.is_finite()) {
            return Err(ConfigError::Invalid {
                var: "LLM_TIMEOUT_SECS",
                value: timeout.to_string(),
            });
        }
        let temperature = match get("LLM_TEMPERATURE") {
            None => None,
            Some(v) => Some(
                v.parse::<f32>()
                    .ok()
                    .filter(|t| (0.0..=2.0).contains(t))
                    .ok_or(ConfigError::Invalid {
                        var: "LLM_TEMPERATURE",
                        value: v,
                    })?,
            ),
        };
        let json_mode = match get("LLM_JSON_MODE").as_deref() {
            None => false,
            Some("1" | "true" | "yes" | "on") => true,
            Some("0" | "false" | "no" | "off") => false,
            Some(v) => {
                return Err(ConfigError::Invalid {
                    var: "LLM_JSON_MODE",
                    value: v.to_string(),
                });
            }
        };
        Ok(Some(Self {
            endpoint: endpoint(&url),
            api_key: get("LLM_API_KEY"),
            model,
            timeout: Duration::from_secs_f32(timeout),
            max_calls_per_hour: number(
                "LLM_MAX_CALLS_PER_HOUR",
                get("LLM_MAX_CALLS_PER_HOUR"),
                120,
            )?,
            max_tokens: number("LLM_MAX_TOKENS", get("LLM_MAX_TOKENS"), 800)?,
            temperature,
            json_mode,
            max_retries: number("LLM_MAX_RETRIES", get("LLM_MAX_RETRIES"), 1)?,
        }))
    }
}

/// `…/v1` → `…/v1/chat/completions`; a full endpoint is kept as it is.
fn endpoint(url: &str) -> String {
    let url = url.trim_end_matches('/');
    if url.ends_with("/chat/completions") {
        url.to_string()
    } else {
        format!("{url}/chat/completions")
    }
}

/// The nearest `.env`, from the current directory upwards.
fn find_dotenv() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors()
        .map(|dir: &Path| dir.join(".env"))
        .find(|p| p.is_file())
}

/// Parses a `.env` file: `KEY=value` lines, `#` comments, an optional
/// `export ` prefix, values in single or double quotes (double quotes
/// understand `\n`, `\"` and `\\`), and ` #` comments after unquoted values.
pub fn parse_dotenv(text: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        let value = value.trim();
        let value = if let Some(rest) = value.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => out.push('\n'),
                        Some(other) => out.push(other),
                        None => break,
                    },
                    c => out.push(c),
                }
            }
            out
        } else if let Some(rest) = value.strip_prefix('\'') {
            rest.split('\'').next().unwrap_or_default().to_string()
        } else {
            match value.find(" #") {
                Some(i) => value[..i].trim_end().to_string(),
                None => value.to_string(),
            }
        };
        vars.insert(key.to_string(), value);
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> Result<Option<LlmConfig>, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        LlmConfig::from_vars(|k| map.get(k).cloned())
    }

    #[test]
    fn no_url_means_no_ai() {
        assert_eq!(config(&[]), Ok(None));
        assert_eq!(
            config(&[("LLM_API_URL", "  "), ("LLM_MODEL", "m")]),
            Ok(None)
        );
    }

    #[test]
    fn defaults_and_endpoint() {
        let c = config(&[
            ("LLM_API_URL", "https://api.example.com/v1/"),
            ("LLM_MODEL", "m"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(c.endpoint, "https://api.example.com/v1/chat/completions");
        assert_eq!(c.api_key, None);
        assert_eq!(c.timeout, Duration::from_secs(60));
        assert_eq!(c.max_calls_per_hour, 120);
        assert_eq!(c.max_tokens, 800);
        assert_eq!(c.temperature, None);
        assert!(!c.json_mode);
        assert_eq!(c.max_retries, 1);
        let full = config(&[
            ("LLM_API_URL", "http://localhost:11434/v1/chat/completions"),
            ("LLM_MODEL", "m"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(full.endpoint, "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn every_option_is_read() {
        let c = config(&[
            ("LLM_API_URL", "http://x/v1"),
            ("LLM_API_KEY", "sk-secret"),
            ("LLM_MODEL", "big"),
            ("LLM_TIMEOUT_SECS", "2.5"),
            ("LLM_MAX_CALLS_PER_HOUR", "10"),
            ("LLM_MAX_TOKENS", "300"),
            ("LLM_TEMPERATURE", "0.7"),
            ("LLM_JSON_MODE", "true"),
            ("LLM_MAX_RETRIES", "0"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(c.api_key.as_deref(), Some("sk-secret"));
        assert_eq!(c.timeout, Duration::from_millis(2500));
        assert_eq!(
            (c.max_calls_per_hour, c.max_tokens, c.max_retries),
            (10, 300, 0)
        );
        assert_eq!(c.temperature, Some(0.7));
        assert!(c.json_mode);
    }

    #[test]
    fn bad_values_are_reported() {
        let url = ("LLM_API_URL", "http://x/v1");
        assert_eq!(config(&[url]), Err(ConfigError::Missing("LLM_MODEL")));
        let m = ("LLM_MODEL", "m");
        for (var, value) in [
            ("LLM_TIMEOUT_SECS", "0"),
            ("LLM_TIMEOUT_SECS", "presto"),
            ("LLM_MAX_CALLS_PER_HOUR", "-1"),
            ("LLM_TEMPERATURE", "5"),
            ("LLM_JSON_MODE", "forse"),
        ] {
            assert!(
                matches!(
                    config(&[url, m, (var, value)]),
                    Err(ConfigError::Invalid { .. })
                ),
                "{var}={value}"
            );
        }
    }

    #[test]
    fn debug_hides_the_key() {
        let c = config(&[
            ("LLM_API_URL", "http://x/v1"),
            ("LLM_API_KEY", "sk-secret"),
            ("LLM_MODEL", "m"),
        ])
        .unwrap()
        .unwrap();
        let debug = format!("{c:?}");
        assert!(!debug.contains("sk-secret"), "{debug}");
        assert!(debug.contains("***"));
    }

    #[test]
    fn dotenv_syntax() {
        let vars = parse_dotenv(
            "# commento\n\
             LLM_API_URL=https://api.example.com/v1\n\
             export LLM_MODEL = \"modello grande\"\n\
             LLM_API_KEY='sk-#123'\n\
             LLM_MAX_TOKENS=500 # commento in coda\n\
             MULTI=\"riga1\\nriga2 \\\"citata\\\"\"\n\
             senza uguale\n\
             =vuoto\n",
        );
        assert_eq!(vars["LLM_API_URL"], "https://api.example.com/v1");
        assert_eq!(vars["LLM_MODEL"], "modello grande");
        assert_eq!(vars["LLM_API_KEY"], "sk-#123");
        assert_eq!(vars["LLM_MAX_TOKENS"], "500");
        assert_eq!(vars["MULTI"], "riga1\nriga2 \"citata\"");
        assert_eq!(vars.len(), 5);
    }
}
