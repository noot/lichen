use eyre::{Result, WrapErr as _};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Provider {
    Openai,
    Anthropic,
}

#[derive(Clone)]
pub struct LlmClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    provider: Provider,
}

#[derive(Debug, Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    max_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: String,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<AnthropicMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl LlmClient {
    pub fn new(
        base_url: String,
        model: String,
        api_key: Option<String>,
        provider: Provider,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            api_key,
            provider,
        }
    }

    /// Maximum number of retries for rate-limited (429) requests.
    const MAX_RETRIES: u32 = 5;

    /// Base delay for exponential backoff on 429s.
    const RETRY_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

    /// Returns the appropriate max_tokens for this model.
    fn max_tokens(&self) -> u32 {
        let m = self.model.as_str();
        if m.starts_with("gpt-4o")
            || m.starts_with("gpt-4.1-nano")
            || m.starts_with("o3-mini")
            || m.starts_with("o4-mini")
        {
            4096
        } else {
            16384
        }
    }

    pub async fn chat(&self, messages: &[Message]) -> Result<String> {
        let mut last_err = None;
        for attempt in 0..=Self::MAX_RETRIES {
            let result = match self.provider {
                Provider::Openai => self.chat_openai_once(messages).await,
                Provider::Anthropic => self.chat_anthropic_once(messages).await,
            };
            match result {
                Ok(text) => return Ok(text),
                Err(e) => {
                    let is_rate_limited = e.to_string().contains("429");
                    if is_rate_limited && attempt < Self::MAX_RETRIES {
                        #[allow(clippy::arithmetic_side_effects)]
                        let delay = Self::RETRY_BASE_DELAY * 2u32.pow(attempt);
                        #[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
                        {
                            tracing::warn!(
                                model = %self.model,
                                attempt = attempt + 1,
                                delay_ms = delay.as_millis() as u64,
                                "rate limited (429), retrying after backoff"
                            );
                        }
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| eyre::eyre!("max retries exceeded")))
    }

    async fn chat_openai_once(&self, messages: &[Message]) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.client.post(&url).json(&OpenAiRequest {
            model: &self.model,
            messages,
            max_tokens: self.max_tokens(),
        });

        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.wrap_err("failed to send openai request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_else(|_| "<unreadable>".into());
            return Err(eyre::eyre!(
                "openai request failed: model={} status={} body={}",
                self.model,
                status,
                body
            ));
        }
        let body = resp
            .text()
            .await
            .wrap_err("failed to read openai response body")?;
        let resp: OpenAiResponse = serde_json::from_str(&body)
            .wrap_err_with(|| format!("failed to parse openai response: {body}"))?;

        resp.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| eyre::eyre!("empty response from LLM"))
    }

    async fn chat_anthropic_once(&self, messages: &[Message]) -> Result<String> {
        let url = format!("{}/messages", self.base_url);

        let anthropic_messages: Vec<AnthropicMessage<'_>> = messages
            .iter()
            .map(|m| AnthropicMessage {
                role: &m.role,
                content: &m.content,
            })
            .collect();

        let body = AnthropicRequest {
            model: &self.model,
            max_tokens: self.max_tokens(),
            messages: anthropic_messages,
        };

        let mut req = self
            .client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .json(&body);

        if let Some(key) = &self.api_key {
            req = req.header("x-api-key", key);
        }

        let resp = req
            .send()
            .await
            .wrap_err("failed to send anthropic request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_else(|_| "<unreadable>".into());
            return Err(eyre::eyre!(
                "anthropic request failed: model={} status={} body={}",
                self.model,
                status,
                body
            ));
        }
        let body = resp
            .text()
            .await
            .wrap_err("failed to read anthropic response body")?;
        let resp: AnthropicResponse = serde_json::from_str(&body)
            .wrap_err_with(|| format!("failed to parse anthropic response: {body}"))?;

        resp.content
            .into_iter()
            .next()
            .map(|c| c.text)
            .ok_or_else(|| eyre::eyre!("empty response from Anthropic"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Integration test: hits a real LLM API.
    /// Requires LLM_API_KEY and optionally LLM_BASE_URL / LLM_MODEL / LLM_PROVIDER.
    /// Skipped by default; run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn llm_roundtrip() {
        let key = std::env::var("LLM_API_KEY").expect("set LLM_API_KEY to run this test");
        let base_url =
            std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com/v1".into());
        let model =
            std::env::var("LLM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into());
        let provider = match std::env::var("LLM_PROVIDER").unwrap_or_default().as_str() {
            "openai" => Provider::Openai,
            _ => Provider::Anthropic,
        };

        let client = LlmClient::new(base_url, model, Some(key), provider);

        let messages = vec![Message {
            role: "user".into(),
            content: "Reply with exactly one word: pong".into(),
        }];

        let resp = client.chat(&messages).await.unwrap();
        assert!(!resp.is_empty(), "got empty response");
        println!("response: {resp}");
        assert!(
            resp.to_lowercase().contains("pong"),
            "expected 'pong', got: {resp}"
        );
    }
}
