//! Anthropic (Claude) synthesizer — https://docs.anthropic.com/en/api/messages
//!
//! Implements [`Synthesizer`] over the Messages API:
//!
//! - `synthesize` builds a grounded answer from retrieved `Prompt::chunks`
//!   and asks Claude to cite chunk ids it used.
//! - `critique` reviews an [`Outcome`] and proposes a [`Lesson`] if the
//!   outcome looks like a mistake worth remembering.
//!
//! The wire format for `critique`'s lesson proposal is plain JSON:
//!
//! ```json
//! { "lesson": { "trigger": "...", "advice": "..." } }
//! ```
//!
//! A response of `{ "lesson": null }` means "no lesson worth keeping".

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thoth_core::{
    Error, Lesson, MemoryKind, MemoryMeta, Outcome, Prompt, Result, Synthesis, Synthesizer,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const DEFAULT_API_VERSION: &str = "2023-06-01";

/// Handle to Anthropic's Messages API.
#[derive(Debug, Clone)]
pub struct AnthropicSynthesizer {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    base_url: String,
    api_version: String,
}

impl AnthropicSynthesizer {
    /// Construct from key + model. Defaults to a 2048-token answer budget.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
            max_tokens: 2048,
            base_url: DEFAULT_BASE_URL.to_string(),
            api_version: DEFAULT_API_VERSION.to_string(),
        }
    }

    /// Sugar: `claude-sonnet-4-6`.
    pub fn claude_sonnet(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "claude-sonnet-4-6")
    }

    /// Sugar: `claude-opus-4-6`.
    pub fn claude_opus(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "claude-opus-4-6")
    }

    /// Read `ANTHROPIC_API_KEY` and build a Claude Sonnet client.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::Config("ANTHROPIC_API_KEY not set".to_string()))?;
        if key.trim().is_empty() {
            return Err(Error::Config("ANTHROPIC_API_KEY is empty".to_string()));
        }
        Ok(Self::claude_sonnet(key))
    }

    /// Override the answer token budget.
    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = max;
        self
    }

    /// Override the base URL (for tests / proxies).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Low-level: issue a Messages request and return the concatenated text
    /// from the response.
    async fn messages(&self, system: &str, user: &str, max_tokens: u32) -> Result<String> {
        let req = MessagesRequest {
            model: &self.model,
            max_tokens,
            system,
            messages: vec![Message {
                role: "user",
                content: user.to_string(),
            }],
        };
        let url = format!("{}/messages", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.api_version)
            .header("content-type", "application/json")
            .json(&req)
            .send()
            .await
            .map_err(provider)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Provider(format!("anthropic {status}: {body}")));
        }
        let body: MessagesResponse = resp.json().await.map_err(provider)?;
        let mut out = String::new();
        for block in body.content {
            if block.kind == "text" {
                out.push_str(&block.text);
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl Synthesizer for AnthropicSynthesizer {
    async fn synthesize(&self, prompt: &Prompt) -> Result<Synthesis> {
        let system = "You are Thoth, a code-aware assistant. Answer the user's \
            question using ONLY the provided context chunks. When you cite a \
            chunk, quote its id in square brackets like [chunk-id]. If the \
            context is insufficient, say so honestly — do not invent facts.";

        let mut user = String::new();
        if !prompt.lessons.is_empty() {
            user.push_str("### Lessons to keep in mind\n");
            for l in &prompt.lessons {
                user.push_str(&format!(
                    "- when {}: {}\n",
                    l.trigger.trim(),
                    l.advice.trim()
                ));
            }
            user.push('\n');
        }
        user.push_str("### Context\n");
        for c in &prompt.chunks {
            user.push_str(&format!(
                "[{}] {}:{}-{}\n{}\n\n",
                c.id,
                c.path.display(),
                c.span.0,
                c.span.1,
                c.body
            ));
        }
        user.push_str("### Question\n");
        user.push_str(prompt.question.trim());

        let max = prompt.max_tokens.unwrap_or(self.max_tokens);
        let answer = self.messages(system, &user, max).await?;

        // Extract cited chunk ids by regex-lite: anything of the form
        // `[id]` where `id` matches one of the input chunk ids.
        let mut citations = Vec::new();
        for c in &prompt.chunks {
            let needle = format!("[{}]", c.id);
            if answer.contains(&needle) && !citations.contains(&c.id) {
                citations.push(c.id.clone());
            }
        }

        Ok(Synthesis {
            answer,
            citations,
            tokens_used: None,
        })
    }

    async fn critique(&self, outcome: &Outcome) -> Result<Option<Lesson>> {
        let system = "You are a reviewer deciding whether a single outcome \
            warrants a durable LESSON for future sessions. Respond with a \
            single JSON object: \
            `{\"lesson\":{\"trigger\":string,\"advice\":string}}` or \
            `{\"lesson\":null}`. A lesson is worth keeping only if it \
            describes a non-obvious pattern a future agent would likely miss.";

        let user = format!("Outcome:\n{}", outcome_summary(outcome));

        let raw = self.messages(system, &user, 512).await?;
        let parsed: CritiqueResult = match extract_json(&raw) {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| Error::Provider(e.to_string()))?
            }
            None => {
                // Model replied in prose; treat as "no lesson".
                return Ok(None);
            }
        };
        let Some(prop) = parsed.lesson else {
            return Ok(None);
        };
        if prop.trigger.trim().is_empty() || prop.advice.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: prop.trigger,
            advice: prop.advice,
            success_count: 0,
            failure_count: 0,
            enforcement: Default::default(),
            suggested_enforcement: None,
            block_message: None,
        }))
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

// ---- helpers ---------------------------------------------------------------

fn outcome_summary(o: &Outcome) -> String {
    // `Outcome` in thoth-core carries minimal detail today; format what we
    // have. If `Outcome` grows structured fields we fold them in here.
    format!("{o:#?}")
}

/// Extract the first top-level JSON object from a blob of text — tolerant of
/// code fences and preamble that Claude sometimes adds.
fn extract_json(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_str {
            match b {
                b'\\' => escape = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s0) = start {
                        return Some(s[s0..=i].to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn provider(e: impl std::fmt::Display) -> Error {
    Error::Provider(e.to_string())
}

// ---- wire types ------------------------------------------------------------

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct CritiqueResult {
    lesson: Option<LessonProposal>,
}

#[derive(Deserialize)]
struct LessonProposal {
    trigger: String,
    advice: String,
}

#[cfg(test)]
mod tests {
    use super::extract_json;

    #[test]
    fn extracts_fenced_json() {
        let s = "Here you go:\n```json\n{\"lesson\": null}\n```\ndone.";
        assert_eq!(extract_json(s).unwrap(), "{\"lesson\": null}");
    }

    #[test]
    fn extracts_bare_object() {
        let s = "{\"lesson\":{\"trigger\":\"a\",\"advice\":\"b\"}}";
        assert_eq!(extract_json(s).unwrap(), s);
    }

    #[test]
    fn handles_nested_braces() {
        let s = r#"prose {"a":{"b":"}"}} trailing"#;
        assert_eq!(extract_json(s).unwrap(), r#"{"a":{"b":"}"}}"#);
    }

    #[test]
    fn none_when_no_object() {
        assert!(extract_json("no json here").is_none());
    }
}
