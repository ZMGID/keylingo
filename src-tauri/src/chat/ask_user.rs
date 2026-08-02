use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::mcp::ChatToolDefinition;

pub const ASK_USER_TOOL_NAME: &str = "ask_user";
pub const ASK_USER_PHASE_AWAITING: &str = "awaiting";
pub const ASK_USER_PHASE_ANSWERED: &str = "answered";
pub const ASK_USER_PHASE_SKIPPED: &str = "skipped";
pub const ASK_USER_PHASE_TIMEOUT: &str = "timeout";
pub const ASK_USER_PHASE_CANCELLED: &str = "cancelled";

const MAX_TITLE_CHARS: usize = 120;
const MAX_QUESTIONS: usize = 4;
const MAX_QUESTION_ID_CHARS: usize = 40;
const MAX_QUESTION_PROMPT_CHARS: usize = 500;
const MAX_OPTIONS: usize = 6;
const MAX_OPTION_ID_CHARS: usize = 40;
const MAX_OPTION_LABEL_CHARS: usize = 200;
const MAX_OPTION_DESCRIPTION_CHARS: usize = 500;
const MAX_CUSTOM_TEXT_CHARS: usize = 1200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserQuestion {
    pub id: String,
    pub prompt: String,
    pub options: Vec<AskUserOption>,
    #[serde(default)]
    pub allow_multiple: bool,
    #[serde(default)]
    pub allow_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserPromptPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub questions: Vec<AskUserQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserAnswer {
    #[serde(default, alias = "selectedOptionIds")]
    pub selected_option_ids: Vec<String>,
    #[serde(default, alias = "customText")]
    pub custom_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserResponseResult {
    pub phase: String,
    #[serde(default)]
    pub answers: HashMap<String, AskUserAnswer>,
}

pub struct PendingAskUserPrompt {
    pub conversation_id: String,
    pub run_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub prompt: AskUserPromptPayload,
    pub sender: oneshot::Sender<AskUserResponseResult>,
}

pub fn is_ask_user_tool_name(name: &str) -> bool {
    name == ASK_USER_TOOL_NAME
}

pub fn append_tool_definitions(tools: &mut Vec<ChatToolDefinition>) {
    let tool = ask_user_tool();
    if !tools
        .iter()
        .any(|existing| existing.openai_tool_name() == tool.openai_tool_name())
    {
        tools.push(tool);
    }
}

pub fn ask_user_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__ask_user".to_string(),
        name: ASK_USER_TOOL_NAME.to_string(),
        description: "Ask the user blocking clarification questions with concrete answer options, then continue this same run from the structured answer.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: input_schema(),
        sensitive: false,
        annotations: Some(serde_json::json!({
            "readOnlyHint": true,
            "destructiveHint": false,
            "openWorldHint": false
        })),
        output_schema: Some(output_schema()),
    }
}

pub fn normalize_prompt(arguments: Value) -> Result<AskUserPromptPayload, String> {
    let mut prompt: AskUserPromptPayload = serde_json::from_value(arguments)
        .map_err(|err| format!("Invalid ask_user arguments: {err}"))?;
    prompt.title = prompt
        .title
        .map(|title| truncate_clean(title, MAX_TITLE_CHARS))
        .filter(|title| !title.is_empty());

    if prompt.questions.is_empty() {
        return Err("ask_user requires at least one question".to_string());
    }
    if prompt.questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "ask_user supports at most {MAX_QUESTIONS} questions"
        ));
    }

    let mut question_ids = HashSet::new();
    for question in &mut prompt.questions {
        question.id = truncate_clean(question.id.clone(), MAX_QUESTION_ID_CHARS);
        question.prompt = truncate_clean(question.prompt.clone(), MAX_QUESTION_PROMPT_CHARS);
        if question.id.is_empty() {
            return Err("ask_user question id cannot be empty".to_string());
        }
        if question.prompt.is_empty() {
            return Err(format!(
                "ask_user question `{}` prompt cannot be empty",
                question.id
            ));
        }
        if !question_ids.insert(question.id.clone()) {
            return Err(format!(
                "ask_user question id must be unique: {}",
                question.id
            ));
        }
        if question.options.len() < 2 {
            return Err(format!(
                "ask_user question `{}` requires at least 2 options",
                question.id
            ));
        }
        if question.options.len() > MAX_OPTIONS {
            return Err(format!(
                "ask_user question `{}` supports at most {MAX_OPTIONS} options",
                question.id
            ));
        }

        let mut option_ids = HashSet::new();
        for option in &mut question.options {
            option.id = truncate_clean(option.id.clone(), MAX_OPTION_ID_CHARS);
            option.label = truncate_clean(option.label.clone(), MAX_OPTION_LABEL_CHARS);
            option.description = option
                .description
                .take()
                .map(|description| truncate_clean(description, MAX_OPTION_DESCRIPTION_CHARS))
                .filter(|description| !description.is_empty());
            if option.id.is_empty() {
                return Err(format!(
                    "ask_user question `{}` has an empty option id",
                    question.id
                ));
            }
            if option.label.is_empty() {
                return Err(format!(
                    "ask_user question `{}` option `{}` label cannot be empty",
                    question.id, option.id
                ));
            }
            if !option_ids.insert(option.id.clone()) {
                return Err(format!(
                    "ask_user question `{}` option id must be unique: {}",
                    question.id, option.id
                ));
            }
        }
    }

    Ok(prompt)
}

pub fn validate_response(
    prompt: &AskUserPromptPayload,
    response: AskUserResponseResult,
) -> Result<AskUserResponseResult, String> {
    match response.phase.as_str() {
        ASK_USER_PHASE_SKIPPED | ASK_USER_PHASE_TIMEOUT | ASK_USER_PHASE_CANCELLED => {
            return Ok(AskUserResponseResult {
                phase: response.phase,
                answers: HashMap::new(),
            });
        }
        ASK_USER_PHASE_ANSWERED => {}
        other => {
            return Err(format!("Invalid ask_user response phase: {other}"));
        }
    }

    let mut answers = HashMap::new();
    for question in &prompt.questions {
        let answer = response
            .answers
            .get(&question.id)
            .ok_or_else(|| format!("Missing answer for question `{}`", question.id))?;
        answers.insert(question.id.clone(), normalize_answer(question, answer)?);
    }

    Ok(AskUserResponseResult {
        phase: ASK_USER_PHASE_ANSWERED.to_string(),
        answers,
    })
}

pub fn skipped_response() -> AskUserResponseResult {
    phase_response(ASK_USER_PHASE_SKIPPED)
}

pub fn timeout_response() -> AskUserResponseResult {
    phase_response(ASK_USER_PHASE_TIMEOUT)
}

pub fn cancelled_response() -> AskUserResponseResult {
    phase_response(ASK_USER_PHASE_CANCELLED)
}

pub fn structured_content(
    prompt: &AskUserPromptPayload,
    phase: &str,
    answers: &HashMap<String, AskUserAnswer>,
) -> Value {
    serde_json::json!({
        "askUser": {
            "phase": phase,
            "title": prompt.title,
            "questions": prompt.questions,
            "answers": answers,
        }
    })
}

pub fn tool_result_content(response: &AskUserResponseResult) -> String {
    serde_json::to_string(&serde_json::json!({
        "phase": response.phase,
        "answers": response.answers,
    }))
    .unwrap_or_else(|_| "{\"phase\":\"error\",\"answers\":{}}".to_string())
}

pub fn format_prompt(available: bool) -> String {
    if available {
        "Structured clarification: use the ask_user tool when a product decision, scope choice, or user preference would block useful work in this same run. If the user asks you to wait for a choice, explicitly requests ask_user, or presents A/B/C options that block later steps, you must call ask_user instead of listing options in assistant text for the user to type back. Do not ask questions that can be answered by reading files, inspecting context, using available tools, or searching. Prefer 1-3 high-value questions with concrete actionable options; allow custom text only when realistic options may not cover the user's intent.".to_string()
    } else {
        "The structured clarification tool ask_user is unavailable; if clarification is truly required, ask briefly in natural language.".to_string()
    }
}

fn normalize_answer(
    question: &AskUserQuestion,
    answer: &AskUserAnswer,
) -> Result<AskUserAnswer, String> {
    let allowed_options = question
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for option_id in &answer.selected_option_ids {
        let trimmed = option_id.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !allowed_options.contains(trimmed) {
            return Err(format!(
                "Unknown option `{trimmed}` for question `{}`",
                question.id
            ));
        }
        if seen.insert(trimmed.to_string()) {
            selected.push(trimmed.to_string());
        }
    }

    let custom_text = answer
        .custom_text
        .as_ref()
        .map(|text| truncate_clean(text.clone(), MAX_CUSTOM_TEXT_CHARS))
        .filter(|text| !text.is_empty());

    if selected.is_empty() && custom_text.is_none() {
        return Err(format!(
            "Question `{}` requires at least one option or custom answer",
            question.id
        ));
    }
    if !question.allow_multiple && selected.len() > 1 {
        return Err(format!(
            "Question `{}` accepts exactly one selected option",
            question.id
        ));
    }
    if custom_text.is_some() && !question.allow_custom {
        return Err(format!(
            "Question `{}` does not accept custom text",
            question.id
        ));
    }

    Ok(AskUserAnswer {
        selected_option_ids: selected,
        custom_text,
    })
}

fn phase_response(phase: &str) -> AskUserResponseResult {
    AskUserResponseResult {
        phase: phase.to_string(),
        answers: HashMap::new(),
    }
}

fn truncate_clean(value: String, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

fn input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "maxLength": MAX_TITLE_CHARS
            },
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_QUESTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": MAX_QUESTION_ID_CHARS
                        },
                        "prompt": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": MAX_QUESTION_PROMPT_CHARS
                        },
                        "options": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": MAX_OPTIONS,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {
                                        "type": "string",
                                        "minLength": 1,
                                        "maxLength": MAX_OPTION_ID_CHARS
                                    },
                                    "label": {
                                        "type": "string",
                                        "minLength": 1,
                                        "maxLength": MAX_OPTION_LABEL_CHARS
                                    },
                                    "description": {
                                        "type": "string",
                                        "maxLength": MAX_OPTION_DESCRIPTION_CHARS
                                    }
                                },
                                "required": ["id", "label"],
                                "additionalProperties": false
                            }
                        },
                        "allow_multiple": {
                            "type": "boolean",
                            "default": false
                        },
                        "allow_custom": {
                            "type": "boolean",
                            "default": false
                        }
                    },
                    "required": ["id", "prompt", "options"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "phase": {
                "type": "string",
                "enum": [
                    ASK_USER_PHASE_ANSWERED,
                    ASK_USER_PHASE_SKIPPED,
                    ASK_USER_PHASE_TIMEOUT,
                    ASK_USER_PHASE_CANCELLED
                ]
            },
            "answers": {
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "properties": {
                        "selected_option_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "custom_text": {
                            "type": ["string", "null"]
                        }
                    },
                    "required": ["selected_option_ids"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["phase", "answers"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt_args() -> Value {
        serde_json::json!({
            "title": "Scope",
            "questions": [
                {
                    "id": "surface",
                    "prompt": "Where should the UI live?",
                    "options": [
                        { "id": "inline", "label": "Inline" },
                        { "id": "modal", "label": "Modal" }
                    ],
                    "allow_custom": true
                },
                {
                    "id": "modes",
                    "prompt": "Which modes are required?",
                    "options": [
                        { "id": "single", "label": "Single" },
                        { "id": "multi", "label": "Multi" }
                    ],
                    "allow_multiple": true
                }
            ]
        })
    }

    #[test]
    fn normalize_rejects_missing_questions() {
        let err =
            normalize_prompt(serde_json::json!({})).expect_err("missing questions should fail");
        assert!(err.contains("questions"));
    }

    #[test]
    fn normalize_rejects_too_many_questions() {
        let err = normalize_prompt(serde_json::json!({
            "questions": [
                { "id": "a", "prompt": "A", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] },
                { "id": "b", "prompt": "B", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] },
                { "id": "c", "prompt": "C", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] },
                { "id": "d", "prompt": "D", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] },
                { "id": "e", "prompt": "E", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] }
            ]
        }))
        .expect_err("too many questions should fail");
        assert!(err.contains("at most"));
    }

    #[test]
    fn normalize_rejects_too_few_options() {
        let err = normalize_prompt(serde_json::json!({
            "questions": [
                { "id": "a", "prompt": "A", "options": [{ "id": "x", "label": "X" }] }
            ]
        }))
        .expect_err("too few options should fail");
        assert!(err.contains("at least 2"));
    }

    #[test]
    fn normalize_rejects_duplicate_ids() {
        let err = normalize_prompt(serde_json::json!({
            "questions": [
                { "id": "a", "prompt": "A", "options": [{ "id": "x", "label": "X" }, { "id": "x", "label": "Y" }] }
            ]
        }))
        .expect_err("duplicate option ids should fail");
        assert!(err.contains("unique"));

        let err = normalize_prompt(serde_json::json!({
            "questions": [
                { "id": "a", "prompt": "A", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] },
                { "id": "a", "prompt": "B", "options": [{ "id": "x", "label": "X" }, { "id": "y", "label": "Y" }] }
            ]
        }))
        .expect_err("duplicate question ids should fail");
        assert!(err.contains("unique"));
    }

    #[test]
    fn validate_accepts_single_multi_and_custom() {
        let prompt = normalize_prompt(prompt_args()).expect("prompt");
        let response = validate_response(
            &prompt,
            AskUserResponseResult {
                phase: ASK_USER_PHASE_ANSWERED.to_string(),
                answers: HashMap::from([
                    (
                        "surface".to_string(),
                        AskUserAnswer {
                            selected_option_ids: vec!["inline".to_string()],
                            custom_text: Some("Timeline card".to_string()),
                        },
                    ),
                    (
                        "modes".to_string(),
                        AskUserAnswer {
                            selected_option_ids: vec!["single".to_string(), "multi".to_string()],
                            custom_text: None,
                        },
                    ),
                ]),
            },
        )
        .expect("valid response");

        assert_eq!(response.phase, ASK_USER_PHASE_ANSWERED);
        assert_eq!(
            response.answers["surface"].selected_option_ids,
            vec!["inline".to_string()]
        );
    }

    #[test]
    fn validate_rejects_single_question_multiple_options() {
        let prompt = normalize_prompt(prompt_args()).expect("prompt");
        let err = validate_response(
            &prompt,
            AskUserResponseResult {
                phase: ASK_USER_PHASE_ANSWERED.to_string(),
                answers: HashMap::from([
                    (
                        "surface".to_string(),
                        AskUserAnswer {
                            selected_option_ids: vec!["inline".to_string(), "modal".to_string()],
                            custom_text: None,
                        },
                    ),
                    (
                        "modes".to_string(),
                        AskUserAnswer {
                            selected_option_ids: vec!["single".to_string()],
                            custom_text: None,
                        },
                    ),
                ]),
            },
        )
        .expect_err("single question should reject multiple selected options");

        assert!(err.contains("exactly one"));
    }

    #[test]
    fn validate_rejects_custom_when_disallowed() {
        let prompt = normalize_prompt(prompt_args()).expect("prompt");
        let err = validate_response(
            &prompt,
            AskUserResponseResult {
                phase: ASK_USER_PHASE_ANSWERED.to_string(),
                answers: HashMap::from([
                    (
                        "surface".to_string(),
                        AskUserAnswer {
                            selected_option_ids: vec!["inline".to_string()],
                            custom_text: None,
                        },
                    ),
                    (
                        "modes".to_string(),
                        AskUserAnswer {
                            selected_option_ids: vec!["single".to_string()],
                            custom_text: Some("Other".to_string()),
                        },
                    ),
                ]),
            },
        )
        .expect_err("custom text should be rejected when disallowed");

        assert!(err.contains("does not accept custom"));
    }

    #[test]
    fn validate_accepts_skipped_without_answers() {
        let prompt = normalize_prompt(prompt_args()).expect("prompt");
        let response = validate_response(&prompt, skipped_response()).expect("skipped");

        assert_eq!(response.phase, ASK_USER_PHASE_SKIPPED);
        assert!(response.answers.is_empty());
    }
}
