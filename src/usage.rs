//! Reported counts for one returned response, never a task total or budget.
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ResponseUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
}

impl ResponseUsage {
    pub(crate) fn from_openai(response: &Value) -> Option<Self> {
        let usage = response.get("usage")?.as_object()?;
        let input_tokens = usage.get("prompt_tokens")?.as_u64()?;
        let output_tokens = usage.get("completion_tokens")?.as_u64()?;
        let total = input_tokens.checked_add(output_tokens)?;
        if let Some(value) = usage.get("total_tokens") {
            if value.as_u64()? != total {
                return None;
            }
        }
        let cached_input_tokens = match usage.get("prompt_tokens_details") {
            None => None,
            Some(details) => match details.as_object()?.get("cached_tokens") {
                None => None,
                Some(count) => Some(count.as_u64()?),
            },
        };
        if cached_input_tokens.is_some_and(|cached| cached > input_tokens) {
            return None;
        }
        Some(Self {
            input_tokens,
            output_tokens,
            cached_input_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reported_zero_is_distinct_from_missing_or_partial_usage() {
        let zero =
            ResponseUsage::from_openai(&json!({"usage":{"prompt_tokens":0,"completion_tokens":0}}))
                .unwrap();
        assert_eq!(zero.input_tokens, 0);
        assert_eq!(zero.cached_input_tokens, None);
        for usage in [
            json!(null),
            json!({}),
            json!({"prompt_tokens":3}),
            json!({"prompt_tokens":-1,"completion_tokens":2}),
            json!({"prompt_tokens":1.5,"completion_tokens":2}),
            json!({"prompt_tokens":1,"completion_tokens":2,"total_tokens":4}),
            json!({"prompt_tokens":1,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":2}}),
            json!({"prompt_tokens":u64::MAX,"completion_tokens":1}),
        ] {
            assert!(ResponseUsage::from_openai(&json!({"usage":usage})).is_none());
        }
        let value = ResponseUsage::from_openai(&json!({"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":3}}})).unwrap();
        assert_eq!(value.cached_input_tokens, Some(3));
    }
}
