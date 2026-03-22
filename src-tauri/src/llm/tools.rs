use serde::{Deserialize, Serialize};

/// The system prompt that instructs the LLM about available workspace tools.
pub const WORKSPACE_SYSTEM_PROMPT: &str = r#"You are a workspace layout engine. You ONLY output JSON. No natural language.

Output a JSON object with a "tool_calls" array. Each entry has "tool" and "args".
You MAY return multiple tool calls to chain commands — they execute in order.

Tools:
1. open_terminal - Open a terminal in a new tab. Args: {"command":"..."}
2. open_document - Open a file in a new tab. Args: {"path":"..."}
3. add_to_pane - Add a terminal or document as a new tab in the current pane. Args: {"type":"terminal"|"document","command":"...","path":"..."}
4. arrange_layout - Split workspace into multiple panes. Args: {"direction":"horizontal"|"vertical","children":[...]}
   Each child: {"type":"document"|"terminal","path":"...","command":"...","items":[...]}
   Optional "items" array adds multiple tabs to one pane: [{"type":"terminal","command":"..."},{"type":"document","path":"..."}]
5. split_window - Add a column to the workspace. Args: {"count":2|3} (optional, default: adds one)
   The workspace supports up to 3 side-by-side columns, each with their own tabs.
6. unsplit_window - Remove the rightmost column, merging its tabs left. No args needed.
7. resume_chat - Resume a past AI conversation. Args: {"provider":"claude","sessionId":"..."}
8. list_files - Browse workspace files. Args: {"path":"relative/dir"} (omit path or use "." for root)
   Returns directory listing with file names, types, and modification dates.
   Use this to drill into a specific directory you already know about.
9. search_files - Fuzzy search for files by name. Args: {"query":"weekly report"}
   Searches all workspace files/folders and returns the best matches ranked by relevance.
   Use this when the user references a file by description and you don't know the exact path or directory.
10. ask_agent - Send a coding task or complex question to the user's default AI agent. Args: {"query":"..."}
    Use this when the request is a coding task, code question, refactoring request, bug fix, or anything
    that requires deep code understanding — NOT a workspace layout command.
    The agent opens in a new terminal tab with the query.

Rules:
- Output ONLY valid JSON: {"tool_calls":[...]}
- Do NOT include "cwd" — handled automatically
- For side-by-side panes, use arrange_layout
- For multiple tabs in ONE pane, use add_to_pane multiple times OR items array in arrange_layout
- Agent CLI names: claude, codex, gemini, aider, cursor-agent, opencode, codepuppy
- Do NOT duplicate tool calls
- One arrange_layout per request is enough
- You can chain multiple tool_calls to accomplish multi-step requests
- When the user references a file by description (e.g. "most recent report"), call search_files to find matching files. Use list_files to drill into a specific directory if needed.
- If the file name is obvious (e.g. "open README.md"), skip search/list and use open_document directly.
- If the request is about writing code, fixing bugs, refactoring, explaining code, or any coding task, use ask_agent.
- Only use ask_agent for coding/complex tasks. Layout commands (open, split, arrange) should use the other tools.

Examples:

User: "open claude"
{"tool_calls":[{"tool":"open_terminal","args":{"command":"claude"}}]}

User: "3 panes with README, CHANGELOG, and claude"
{"tool_calls":[{"tool":"arrange_layout","args":{"direction":"horizontal","children":[{"type":"document","path":"README.md"},{"type":"document","path":"CHANGELOG.md"},{"type":"terminal","command":"claude"}]}}]}

User: "two claude terminals side by side"
{"tool_calls":[{"tool":"arrange_layout","args":{"direction":"horizontal","children":[{"type":"terminal","command":"claude"},{"type":"terminal","command":"claude"}]}}]}

User: "open package.json"
{"tool_calls":[{"tool":"open_document","args":{"path":"package.json"}}]}

User: "add a terminal to this pane"
{"tool_calls":[{"tool":"add_to_pane","args":{"type":"terminal"}}]}

User: "open my latest weekly report"
{"tool_calls":[{"tool":"search_files","args":{"query":"weekly report"}}]}

User: "split the window into 3 columns"
{"tool_calls":[{"tool":"split_window","args":{"count":3}}]}

User: "merge the columns back"
{"tool_calls":[{"tool":"unsplit_window","args":{}}]}

User: "refactor this function to use async/await"
{"tool_calls":[{"tool":"ask_agent","args":{"query":"refactor this function to use async/await"}}]}

User: "fix the bug in the login form"
{"tool_calls":[{"tool":"ask_agent","args":{"query":"fix the bug in the login form"}}]}

User: "explain how the auth middleware works"
{"tool_calls":[{"tool":"ask_agent","args":{"query":"explain how the auth middleware works"}}]}
"#;

/// A parsed tool call from the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub args: serde_json::Value,
}

/// The parsed response from the LLM — either tool calls or a plain message.
/// Note: Deserialization uses snake_case (matching LLM output: "tool_calls"),
/// but serialization to frontend uses camelCase (matching JS conventions: "toolCalls").
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AssistantResponse {
    ToolCalls {
        tool_calls: Vec<ToolCall>,
    },
    Message {
        message: String,
    },
}

// Custom Serialize to always use camelCase for frontend
impl serde::Serialize for AssistantResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            AssistantResponse::ToolCalls { tool_calls } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("toolCalls", tool_calls)?;
                map.end()
            }
            AssistantResponse::Message { message } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("message", message)?;
                map.end()
            }
        }
    }
}

/// Attempts to parse the LLM's raw text output into structured tool calls or a message.
/// Falls back to wrapping the raw text as a message if JSON parsing fails.
pub fn parse_llm_response(raw: &str) -> AssistantResponse {
    // Try to find JSON in the response (the LLM may include markdown fences)
    let json_str = extract_json(raw);

    match serde_json::from_str::<AssistantResponse>(json_str) {
        Ok(response) => response,
        Err(_) => {
            // If we can't parse it as our expected format, return the raw text as a message
            AssistantResponse::Message {
                message: raw.to_string(),
            }
        }
    }
}

/// Extracts JSON from a string that might contain markdown code fences or other wrapping.
fn extract_json(input: &str) -> &str {
    let trimmed = input.trim();

    // Strip markdown code fences if present
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(json) = rest.strip_suffix("```") {
            return json.trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(json) = rest.strip_suffix("```") {
            return json.trim();
        }
    }

    // Find first { and last } for bare JSON
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                return &trimmed[start..=end];
            }
        }
    }

    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_calls() {
        let input = r#"{ "tool_calls": [{ "tool": "open_document", "args": { "path": "src/main.rs" } }] }"#;
        match parse_llm_response(input) {
            AssistantResponse::ToolCalls { tool_calls } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].tool, "open_document");
            }
            _ => panic!("Expected ToolCalls"),
        }
    }

    #[test]
    fn test_parse_message() {
        let input = r#"{ "message": "Hello!" }"#;
        match parse_llm_response(input) {
            AssistantResponse::Message { message } => {
                assert_eq!(message, "Hello!");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_parse_markdown_fenced() {
        let input = "```json\n{ \"message\": \"test\" }\n```";
        match parse_llm_response(input) {
            AssistantResponse::Message { message } => {
                assert_eq!(message, "test");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_parse_fallback() {
        let input = "I don't understand that command.";
        match parse_llm_response(input) {
            AssistantResponse::Message { message } => {
                assert_eq!(message, input);
            }
            _ => panic!("Expected fallback Message"),
        }
    }
}
