//! Token-usage accounting.
//!
//! The PTY backend surfaces the assistant message's raw Anthropic `usage`
//! object (pulled verbatim from the session JSONL). [`CliUsage`] mirrors the
//! fields we care about and converts them into OpenAI's `Usage` shape.

use serde::Deserialize;

/// Token accounting from the transcript. Cache tokens are part of the prompt
/// input. All fields default to 0 so a partial/absent usage object is tolerated.
#[derive(Debug, Default, Deserialize)]
pub struct CliUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

impl CliUsage {
    /// OpenAI `prompt_tokens` counts all input, including cached tokens.
    pub fn prompt_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens
    }

    pub fn to_openai(&self) -> super::types::Usage {
        let prompt = self.prompt_tokens();
        super::types::Usage {
            prompt_tokens: prompt,
            completion_tokens: self.output_tokens,
            total_tokens: prompt + self.output_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_cache_tokens_into_prompt() {
        let u = CliUsage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_input_tokens: 5,
            cache_creation_input_tokens: 3,
        };
        let o = u.to_openai();
        assert_eq!(o.prompt_tokens, 108);
        assert_eq!(o.completion_tokens, 20);
        assert_eq!(o.total_tokens, 128);
    }

    #[test]
    fn parses_from_transcript_value() {
        let v = serde_json::json!({"input_tokens": 7, "output_tokens": 2});
        let u: CliUsage = serde_json::from_value(v).unwrap();
        assert_eq!(u.to_openai().total_tokens, 9);
    }
}
