use eyre::{Result, WrapErr as _};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Provider {
    Openai,
    Anthropic,
}

#[derive(Clone)]
pub(crate) struct LlmClient {
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
pub(crate) struct Message {
    pub(crate) role: String,
    pub(crate) content: String,
}

impl LlmClient {
    pub(crate) fn new(
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

    pub(crate) async fn chat(&self, messages: &[Message]) -> Result<String> {
        match self.provider {
            Provider::Openai => self.chat_openai(messages).await,
            Provider::Anthropic => self.chat_anthropic(messages).await,
        }
    }

    async fn chat_openai(&self, messages: &[Message]) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.client.post(&url).json(&OpenAiRequest {
            model: &self.model,
            messages,
        });

        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.wrap_err("failed to send openai request")?;
        let resp = resp.error_for_status().wrap_err("openai request failed")?;
        let resp: OpenAiResponse = resp
            .json()
            .await
            .wrap_err("failed to parse openai response")?;

        resp.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| eyre::eyre!("empty response from LLM"))
    }

    async fn chat_anthropic(&self, messages: &[Message]) -> Result<String> {
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
            max_tokens: 1024,
            messages: anthropic_messages,
        };

        let mut req = self
            .client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .json(&body);

        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .wrap_err("failed to send anthropic request")?;
        let resp = resp
            .error_for_status()
            .wrap_err("anthropic request failed")?;
        let resp: AnthropicResponse = resp
            .json()
            .await
            .wrap_err("failed to parse anthropic response")?;

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

    /// Integration test: hits a real Anthropic-compatible API.
    /// Requires LLM_API_KEY and optionally LLM_BASE_URL / LLM_MODEL.
    /// Skipped by default; run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn anthropic_roundtrip() {
        let key = std::env::var("LLM_API_KEY").expect("set LLM_API_KEY to run this test");
        let base_url =
            std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com/v1".into());
        let model =
            std::env::var("LLM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into());

        let client = LlmClient::new(base_url, model, Some(key), Provider::Anthropic);

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
