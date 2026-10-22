// Claude API client — the same integration as ClaudeService.swift: multi-turn
// chat with web search, and files sent as text, image, or file content parts.
//
// API requests and file reads stay on the Rust side; key reveal is a separate,
// explicit Settings action.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::{automation, llm_client};

/// Text and code files are inlined; anything larger is skipped, as on macOS.
const MAX_INLINE_TEXT: u64 = 200_000;
const MAX_SCREEN_IMAGE_BYTES: usize = 2_000_000;

pub const DEFAULT_MODEL: &str = "openrouter/free";

const SYSTEM_PROMPT: &str = "You are Macha, a personal AI assistant living at the top of the user's screen. \
You have web search access and can help with absolutely anything — research, coding, finding places, recommendations, tasks, questions. \
When the user explicitly asks to open an installed app, folder, or file, use the local action tools instead of merely explaining how. \
For file edits, write_file replaces all existing contents and erase_file_content clears them; state that clearly. Ordinary chat actions require user approval, but actions in a saved workflow the user explicitly started are pre-authorized and should run automatically. \
Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
Use fenced code blocks for code, commands, configuration, and other copyable snippets; add a language tag when known. \
Keep surrounding prose plain and readable. \
Use the existing configured model and provider; do not claim to use another model. \
Use persistent memory sparingly for durable facts, preferences, goals, and projects; update a listed memory by its ID when it changes, and never save credentials or secrets. \
On Windows, you may propose visible-window inspection, mouse and keyboard input, window switching/resizing, and PowerShell commands for the user's requested task. Ordinary chat actions require explicit approval; actions in a saved workflow the user explicitly started are pre-authorized and should run automatically. PowerShell runs with the user's account permissions, so keep the exact command visible. \
Use open_website or search_web only when useful; these require approval in ordinary chat, while actions in a saved workflow explicitly started by the user are pre-authorized.";
const WORKFLOW_SYSTEM_PROMPT: &str = "\nFor a user-started saved workflow, take ONE action at a time. After each action you will receive a new screenshot. Look at it carefully before deciding anything. \
Never assume an action worked. Confirm it from the screenshot. \
If a Chrome profile chooser is visible, click only the profile named \"1\". If it isn't visible, tell me, don't pick another. \
Never end your turn without either a next action or an explicit COMPLETE / STOPPED_FOR_USER / FAILED(step) signal. \
If the screen doesn't match what you expect, wait and re-check before acting. \
Use only open_app, windows_click, windows_type, windows_shortcut, or windows_scroll for workflow actions. To wait without acting, reply exactly WAIT or WAIT(milliseconds).";

#[derive(Default)]
pub struct Chat {
    /// Full multi-turn history in OpenRouter Chat Completions message format.
    conversations: Mutex<HashMap<String, Vec<Value>>>,
    pending_actions: Mutex<HashMap<String, PendingAction>>,
    requests: Mutex<HashMap<String, watch::Sender<bool>>>,
}

struct PendingAction {
    tool_call_id: Option<String>,
    call_index: usize,
    action: automation::AutomationAction,
    remaining: VecDeque<QueuedAction>,
    results: Vec<QueuedActionResult>,
    workflow_step: bool,
}

struct QueuedAction {
    call_index: usize,
    tool_call_id: String,
    action: automation::AutomationAction,
}

struct QueuedActionResult {
    call_index: usize,
    tool_call_id: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistoryMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedConversationContext {
    title: String,
    messages: Vec<ChatHistoryMessage>,
    file: Option<SharedContextFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedContextFile {
    name: String,
    path: String,
}

impl Chat {
    pub fn reset(&self) {
        self.conversations.lock().unwrap().clear();
        self.pending_actions.lock().unwrap().clear();
        for request in self.requests.lock().unwrap().values() {
            let _ = request.send(true);
        }
        self.requests.lock().unwrap().clear();
    }

    pub fn delete(&self, conversation_id: &str) {
        self.conversations.lock().unwrap().remove(conversation_id);
        self.pending_actions.lock().unwrap().remove(conversation_id);
    }

    fn seed_history(
        &self,
        conversation_id: &str,
        history: &[ChatHistoryMessage],
        context: Option<&ChatContext>,
        shared_context: &[SharedConversationContext],
    ) -> Result<(), String> {
        if conversation_id.is_empty() {
            return Err("Conversation ID is required.".into());
        }
        let mut conversations = self.conversations.lock().unwrap();
        let messages = conversations
            .entry(conversation_id.to_string())
            .or_default();
        if messages.is_empty() {
            let mut seeded = Vec::with_capacity(history.len());
            let mut context_added = false;
            for message in history {
                let role = match message.role.as_str() {
                    "user" => "user",
                    "assistant" => "assistant",
                    _ => return Err("Invalid conversation history.".into()),
                };
                if role == "user" {
                    let mut content = Vec::new();
                    if !context_added {
                        append_context(&mut content, context, shared_context)?;
                        context_added = true;
                    }
                    content.push(json!({ "type": "text", "text": message.content }));
                    seeded.push(json!({ "role": role, "content": content }));
                } else {
                    seeded.push(json!({
                        "role": role,
                        "content": message.content,
                    }));
                }
            }
            *messages = seeded;
        }
        Ok(())
    }

    fn is_empty(&self, conversation_id: &str) -> bool {
        self.conversations
            .lock()
            .unwrap()
            .get(conversation_id)
            .map_or(true, Vec::is_empty)
    }

    fn push(&self, conversation_id: &str, message: Value) {
        self.conversations
            .lock()
            .unwrap()
            .entry(conversation_id.to_string())
            .or_default()
            .push(message);
    }

    fn pop(&self, conversation_id: &str) {
        if let Some(messages) = self.conversations.lock().unwrap().get_mut(conversation_id) {
            messages.pop();
        }
    }

    fn snapshot(&self, conversation_id: &str) -> Vec<Value> {
        self.conversations
            .lock()
            .unwrap()
            .get(conversation_id)
            .cloned()
            .unwrap_or_default()
    }

    fn take_pending_action(&self, conversation_id: &str) -> Option<PendingAction> {
        self.pending_actions.lock().unwrap().remove(conversation_id)
    }

    pub fn begin_request(&self, request_id: &str) -> Result<watch::Receiver<bool>, String> {
        if request_id.is_empty() || request_id.len() > 100 {
            return Err("Invalid chat request ID.".into());
        }
        let (sender, receiver) = watch::channel(false);
        let mut requests = self.requests.lock().unwrap();
        if requests.contains_key(request_id) {
            return Err("A chat request with this ID is already running.".into());
        }
        requests.insert(request_id.to_string(), sender);
        Ok(receiver)
    }

    pub fn cancel_request(&self, request_id: &str) -> Result<(), String> {
        let requests = self.requests.lock().unwrap();
        let sender = requests
            .get(request_id)
            .ok_or_else(|| "This chat request is no longer running.".to_string())?;
        sender
            .send(true)
            .map_err(|_| "This chat request is no longer running.".to_string())
    }

    pub fn finish_request(&self, request_id: &str) {
        self.requests.lock().unwrap().remove(request_id);
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File {
        name: String,
        path: String,
    },
    Window {
        app_name: String,
        title: String,
        url: Option<String>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
    pub model: String,
    pub action: Option<automation::AutomationAction>,
    pub provider: String,
    pub fallback_notice: Option<String>,
}

/// One chat turn. Returns the assistant's text, or a message the island shows
/// in the note view.
pub async fn send(
    chat: &Chat,
    conversation_id: String,
    history: Vec<ChatHistoryMessage>,
    model: &str,
    query: String,
    context: Option<ChatContext>,
    shared_context: Vec<SharedConversationContext>,
    automation_folders: &[String],
    provider_mode: &str,
    ollama_model: &str,
    screen_image: Option<&str>,
    voice_deltas: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    workflow_phase: Option<&str>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<ChatReply, String> {
    if let Some(image) = screen_image {
        validate_screen_image(image)?;
    }
    let query_preview = query.chars().take(80).collect::<String>();
    crate::log::line(format!("[claude::send] phase={workflow_phase:?} query={query_preview:?}"));
    chat.seed_history(
        &conversation_id,
        &history,
        context.as_ref(),
        &shared_context,
    )?;
    if let Some(app_name) = automation::explicit_open_app(&query) {
        let action = automation::AutomationAction {
            name: "open_app".into(),
            arguments: json!({ "appName": app_name }),
            preview: None,
            preview_hash: None,
        };
        return propose_local_action(chat, &conversation_id, &query, action, automation_folders);
    }
    if let Some(path) = automation::explicit_open_path(&query) {
        let action = automation::AutomationAction {
            name: "open_path".into(),
            arguments: json!({ "path": path }),
            preview: None,
            preview_hash: None,
        };
        return propose_local_action(chat, &conversation_id, &query, action, automation_folders);
    }

    let memories = crate::memory::list()?;
    // File / window context rides along with the first message only, exactly
    // like ClaudeService.chat().
    let mut content: Vec<Value> = Vec::new();
    let is_first_turn = chat.is_empty(&conversation_id);
    if is_first_turn {
        append_context(&mut content, context.as_ref(), &shared_context)?;
    }
    content.push(json!({ "type": "text", "text": query }));

    chat.push(
        &conversation_id,
        json!({ "role": "user", "content": content }),
    );

    let request_messages = with_screen_image(
        provider_messages(
            chat.snapshot(&conversation_id),
            automation_folders,
            &memories,
            workflow_phase.is_some(),
        ),
        screen_image,
    );
    let compatibility_mode = workflow_phase.is_none() && local_file_action_intent(&query);
    let tools = match workflow_phase {
        Some("action") => Some(workflow_action_tools()),
        Some("plan" | "verify") => None,
        _ => Some(assistant_tools()),
    };
    let completion_result = if compatibility_mode {
        crate::log::line(
            "local file action detected; using provider-independent action-envelope compatibility mode",
        );
        llm_client::chat(
            compatibility_messages(request_messages.clone())?,
            None,
            provider_mode,
            ollama_model,
            model,
            cancel,
        )
        .await
    } else if let Some(deltas) = voice_deltas {
        llm_client::chat_stream(
            request_messages.clone(),
            tools.clone(),
            provider_mode,
            ollama_model,
            model,
            cancel,
            deltas,
        )
        .await
    } else {
        llm_client::chat(
            request_messages.clone(),
            tools.clone(),
            provider_mode,
            ollama_model,
            model,
            cancel,
        )
        .await
    };
    let completion = match completion_result {
        Err(error) if tool_support_error(&error) => {
            crate::log::line(
                "configured model does not support function calls; retrying with an action-envelope compatibility prompt",
            );
            if workflow_phase == Some("action") {
                llm_client::chat(
                    compatibility_messages(request_messages.clone())?,
                    None,
                    provider_mode,
                    ollama_model,
                    model,
                    cancel,
                )
                .await
            } else {
                Err(error)
            }
        }
        result => result,
    };
    let completion = match completion {
        Ok(completion)
            if workflow_phase.is_none()
                && !compatibility_mode
                && has_empty_action_response(&completion.response) =>
        {
            crate::log::line(
                "configured model returned an empty tool batch; retrying with an action-envelope compatibility prompt",
            );
            llm_client::chat(
                compatibility_messages(request_messages.clone())?,
                None,
                provider_mode,
                ollama_model,
                model,
                cancel,
            )
            .await
        }
        result => result,
    };
    let completion = match completion {
        Err(error) if is_multimodal_error(&error) && screen_image.is_some() => {
            crate::log::line(
                "configured model does not support images/multimodal; refusing to answer without the selected screen image",
            );
            Err(
                "The selected AI model cannot analyze images. Choose a vision-capable model, then try the screen question again."
                    .into(),
            )
        }
        result => result,
    };
    let completion = match completion {
        Ok(completion) => completion,
        Err(error) => {
            chat.pop(&conversation_id);
            return Err(error);
        }
    };

    let reply = match handle_response_mode(
        chat,
        &conversation_id,
        automation_folders,
        completion,
        workflow_phase == Some("action"),
        workflow_phase == Some("action"),
    ) {
        Ok(reply) => Ok(reply),
        Err(error) => {
            chat.pop(&conversation_id);
            Err(error)
        }
    };
    if let Ok(ref r) = reply {
        let text_preview = r.text.chars().take(100).collect::<String>();
        let action_name = r.action.as_ref().map(|a| a.name.as_str());
        crate::log::line(format!("[claude::send] reply action={action_name:?} text={text_preview:?}"));
    } else if let Err(ref e) = reply {
        crate::log::line(format!("[claude::send] error={e}"));
    }
    reply
}

fn tool_support_error(error: &str) -> bool {
    let message = error.to_ascii_lowercase();
    (message.contains("tool") || message.contains("function"))
        && [
            "unsupported",
            "not supported",
            "doesn't support",
            "does not support",
            "not available",
            "not enabled",
            "not allowed",
            "disabled",
            "no endpoints",
            "invalid",
        ]
        .iter()
        .any(|marker| message.contains(marker))
}

fn is_multimodal_error(error: &str) -> bool {
    let message = error.to_ascii_lowercase();
    message.contains("multimodal")
        || message.contains("does not support image")
        || message.contains("image input")
        || message.contains("images are not supported")
        || message.contains("unsupported image")
        || message.contains("has no image support")
}

fn has_empty_action_response(response: &Value) -> bool {
    let Some(message) = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
    else {
        return false;
    };
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
        && response_text(message).is_empty()
}

fn local_file_action_intent(query: &str) -> bool {
    let mut query = query.trim().to_ascii_lowercase();
    if ["how ", "what ", "why ", "explain ", "tell me how "]
        .iter()
        .any(|prefix| query.starts_with(prefix))
    {
        return false;
    }
    for prefix in ["please ", "can you ", "could you ", "would you "] {
        if let Some(stripped) = query.strip_prefix(prefix) {
            query = stripped.to_string();
            break;
        }
    }
    let starts_with_action = [
        "create ",
        "make ",
        "write ",
        "edit ",
        "update ",
        "replace ",
        "clear ",
        "empty ",
        "erase ",
        "move ",
        "copy ",
        "save ",
        "read ",
        "list ",
        "show ",
        "open ",
        "fix ",
        "organize ",
    ]
    .iter()
    .any(|verb| query.starts_with(verb));
    starts_with_action
        && [
            "file",
            "folder",
            "directory",
            "document",
            "text file",
            "computer",
            "workspace",
            "://",
            ":\\",
            "/",
        ]
        .iter()
        .any(|subject| query.contains(subject))
}

fn compatibility_messages(mut messages: Vec<Value>) -> Result<Vec<Value>, String> {
    let schema = serde_json::to_string(&assistant_tools()).map_err(|error| {
        format!("Could not prepare the model action compatibility prompt: {error}")
    })?;
    let system = messages
        .first_mut()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        .ok_or_else(|| "The chat request is missing its system instructions.".to_string())?;
    let system_content = system
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "The chat system instructions are invalid.".to_string())?;
    system["content"] = json!(format!(
        "{system_content}\n\n\
         Native function calling is unavailable for this configured model. To request one supported action, \
         return exactly one <macha_action>JSON</macha_action> block with this shape: \
         {{\"name\":\"tool_name\",\"arguments\":{{...}}}}. Do not put the block in Markdown fences. \
         Available action definitions: {schema}. \
         A block is only a proposal: Macha validates it and asks the user before performing any action that \
         changes local data or opens an app, path, or website. Never claim the action is already completed. \
         If no action is needed, answer normally without a block."
    ));
    messages.retain_mut(|message| {
        if message.get("role").and_then(Value::as_str) == Some("assistant")
            && message.get("tool_calls").is_some()
        {
            message
                .as_object_mut()
                .map(|object| object.remove("tool_calls"));
            if message
                .get("content")
                .and_then(Value::as_str)
                .map_or(true, str::is_empty)
            {
                return false;
            }
        }
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            let result = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("No action result was returned.");
            *message = json!({
                "role": "assistant",
                "content": format!("Macha action result: {result}"),
            });
        }
        true
    });
    Ok(messages)
}

fn propose_local_action(
    chat: &Chat,
    conversation_id: &str,
    query: &str,
    action: automation::AutomationAction,
    automation_folders: &[String],
) -> Result<ChatReply, String> {
    let action = automation::prepare(action, automation_folders)?;
    chat.push(conversation_id, json!({ "role": "user", "content": query }));
    chat.pending_actions.lock().unwrap().insert(
        conversation_id.to_string(),
        PendingAction {
            tool_call_id: None,
            call_index: 0,
            action: action.clone(),
            remaining: VecDeque::new(),
            results: Vec::new(),
            workflow_step: false,
        },
    );
    Ok(ChatReply {
        text: String::new(),
        model: "Local automation".into(),
        action: Some(action),
        provider: "local".into(),
        fallback_notice: None,
    })
}

pub async fn resolve_action(
    chat: &Chat,
    conversation_id: &str,
    model: &str,
    approved: bool,
    selected_app_id: Option<String>,
    automation_folders: &[String],
    provider_mode: &str,
    ollama_model: &str,
    workflow_step: bool,
    cancel: &mut watch::Receiver<bool>,
) -> Result<ChatReply, String> {
    if approved {
        let mut pending_actions = chat.pending_actions.lock().unwrap();
        let pending = pending_actions
            .get_mut(conversation_id)
            .ok_or_else(|| "This local action is no longer pending.".to_string())?;
        automation::select_app_target(&mut pending.action, selected_app_id.as_deref())?;
    }
    let mut pending = chat
        .take_pending_action(conversation_id)
        .ok_or_else(|| "This local action is no longer pending.".to_string())?;
    let Some(tool_call_id) = pending.tool_call_id else {
        let text = if approved {
            automation::execute(&pending.action, automation_folders)?
        } else if pending.action.name == "open_app" {
            "Okay, I won't open the app. Nothing was changed.".into()
        } else if pending.action.name == "open_path" {
            "Okay, I won't open the file or folder. Nothing was changed.".into()
        } else {
            "Okay, I won't perform that action. Nothing was changed.".into()
        };
        chat.push(
            conversation_id,
            json!({ "role": "assistant", "content": text }),
        );
        return Ok(ChatReply {
            text,
            model: "Local automation".into(),
            action: None,
            provider: "local".into(),
            fallback_notice: None,
        });
    };
    let memories = crate::memory::list()?;
    let result = if approved {
        automation::execute(&pending.action, automation_folders)
            .unwrap_or_else(|error| format!("Action failed: {error}"))
    } else {
        "The user denied this action. Do not repeat it; explain that no change was made.".into()
    };
    crate::log::line(format!(
        "[claude::action] name={} result={:?}",
        pending.action.name,
        result.chars().take(100).collect::<String>()
    ));
    let mut results = pending.results;
    results.push(QueuedActionResult {
        call_index: pending.call_index,
        tool_call_id,
        content: result,
    });
    if let Some(next) = pending.remaining.pop_front() {
        let reply = ChatReply {
            text: String::new(),
            model: "Local automation".into(),
            action: Some(next.action.clone()),
            provider: "local".into(),
            fallback_notice: None,
        };
        chat.pending_actions.lock().unwrap().insert(
            conversation_id.to_string(),
            PendingAction {
                tool_call_id: Some(next.tool_call_id),
                call_index: next.call_index,
                action: next.action,
                remaining: pending.remaining,
                results,
                workflow_step: pending.workflow_step,
            },
        );
        return Ok(reply);
    }
    results.sort_by_key(|result| result.call_index);
    for result in &results {
        chat.push(
            conversation_id,
            json!({
                "role": "tool",
                "tool_call_id": result.tool_call_id,
                "content": result.content,
            }),
        );
    }
    if pending.workflow_step || workflow_step {
        let action_result = results
            .last()
            .map(|result| result.content.as_str())
            .unwrap_or("No action result was returned.");
        let text = format!("Action result: {action_result}");
        chat.push(
            conversation_id,
            json!({ "role": "assistant", "content": text }),
        );
        return Ok(ChatReply {
            text,
            model: "Local automation".into(),
            action: None,
            provider: "local".into(),
            fallback_notice: None,
        });
    }
    let messages = provider_messages(
        chat.snapshot(conversation_id),
        automation_folders,
        &memories,
        false,
    );
    let completion = match llm_client::chat(
        messages.clone(),
        Some(assistant_tools()),
        provider_mode,
        ollama_model,
        model,
        cancel,
    )
    .await
    {
        Err(error) if tool_support_error(&error) => {
            crate::log::line(
                "configured model does not support function calls; retrying action follow-up with compatibility prompt",
            );
            llm_client::chat(
                compatibility_messages(messages)?,
                None,
                provider_mode,
                ollama_model,
                model,
                cancel,
            )
            .await?
        }
        result => result?,
    };
    handle_response(chat, conversation_id, automation_folders, completion)
}

fn provider_messages(
    mut history: Vec<Value>,
    automation_folders: &[String],
    memories: &[crate::memory::Memory],
    workflow: bool,
) -> Vec<Value> {
    const MAX_HISTORY_MESSAGES: usize = 20;
    if history.len() > MAX_HISTORY_MESSAGES {
        history.drain(..history.len() - MAX_HISTORY_MESSAGES);
    }
    let relative_path_context = if automation_folders.is_empty() {
        "Relative paths are based at the user's home folder.".to_string()
    } else {
        format!(
            "For relative paths, try these preferred starting folders first: {}. \
If a relative path is not found there, use the user's home folder.",
            automation_folders.join(", ")
        )
    };
    let automation_context = if cfg!(windows) {
        format!(
            "\nThe user enabled local file access across the computer. Use absolute paths to access any location the current user can access, including other mounted drives. \
{relative_path_context} Never attempt deletion. Windows GUI and PowerShell actions are available for the user's requested task. Ordinary chat actions require explicit approval; actions in a saved workflow explicitly started by the user are pre-authorized and must run automatically, one action at a time. \
PowerShell runs with the user's account permissions and can change the system; do not hide or obfuscate the command."
        )
    } else {
        format!(
            "\nThe user enabled local file access across the computer. Use absolute paths to access any location the current user can access, including other mounted drives. \
{relative_path_context} Never attempt deletion or command execution. \
Ordinary chat actions require explicit user approval. Actions in a saved workflow explicitly started by the user are pre-authorized and should run automatically, one action at a time."
        )
    };
    let mut messages = vec![json!({
        "role": "system",
        "content": format!(
            "{SYSTEM_PROMPT}{automation_context}{}",
            crate::memory::system_context(memories),
        ) + if workflow { WORKFLOW_SYSTEM_PROMPT } else { "" },
    })];
    messages.extend(history);
    messages
}

fn with_screen_image(mut messages: Vec<Value>, image: Option<&str>) -> Vec<Value> {
    let Some(image) = image.filter(|s| !s.trim().is_empty()) else {
        return messages;
    };
    let Some(last_user_message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return messages;
    };
    if let Some(content) = last_user_message
        .get_mut("content")
        .and_then(Value::as_array_mut)
    {
        content.push(json!({
            "type": "image_url",
            "image_url": { "url": image },
        }));
    }
    messages
}

fn validate_screen_image(image: &str) -> Result<(), String> {
    if image.trim().is_empty() {
        return Ok(());
    }
    let encoded = image
        .strip_prefix("data:image/jpeg;base64,")
        .ok_or_else(|| "The screen capture must be a JPEG data URL.".to_string())?;
    if encoded.is_empty()
        || encoded.len() > MAX_SCREEN_IMAGE_BYTES
        || encoded.len() % 4 != 0
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err("The screen capture is invalid or exceeds the 2 MB limit.".into());
    }
    Ok(())
}

fn assistant_tools() -> Value {
    let mut tools = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "remember_user_fact",
                "description": "Save or update a durable user-provided fact, preference, goal, project, relationship, or personal context for future conversations. When correcting a listed memory, pass its ID. Use sparingly and never store credentials, passwords, access tokens, payment details, or security answers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "category": {
                            "type": "string",
                            "enum": ["identity", "preference", "goal", "project", "relationship", "emotional", "behavior"]
                        },
                        "text": {
                            "type": "string",
                            "description": "A concise, factual statement about the user."
                        },
                        "id": {
                            "type": "string",
                            "description": "Optional ID of the existing memory to update."
                        }
                    },
                    "required": ["category", "text"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "open_app",
                "description": "Open an installed app by its exact or unambiguous partial name. Requires explicit user approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": { "appName": { "type": "string" } },
                    "required": ["appName"],
                    "additionalProperties": false
                }
            }
        }),
    ];
    tools.extend(automation_tools().as_array().into_iter().flatten().cloned());
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "open_website",
            "description": "Open a public HTTP or HTTPS website in the system browser. Requires explicit user approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
            "parameters": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"],
                "additionalProperties": false
            }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "search_web",
            "description": "Prepare a web search in the system browser. Requires explicit user approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "engine": {
                        "type": "string",
                        "enum": ["google", "youtube", "github", "duckduckgo", "bing"]
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }
    }));
    Value::Array(tools)
}

fn workflow_action_tools() -> Value {
    let mut tools = assistant_tools();
    if let Some(tools) = tools.as_array_mut() {
        tools.retain(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .is_some_and(is_workflow_action)
        });
    }
    tools
}

fn is_workflow_action(name: &str) -> bool {
    matches!(
        name,
        "open_app" | "windows_click" | "windows_type" | "windows_shortcut" | "windows_scroll"
    )
}

fn handle_response(
    chat: &Chat,
    conversation_id: &str,
    automation_folders: &[String],
    completion: llm_client::ChatReply,
) -> Result<ChatReply, String> {
    handle_response_mode(
        chat,
        conversation_id,
        automation_folders,
        completion,
        true,
        false,
    )
}

fn handle_response_mode(
    chat: &Chat,
    conversation_id: &str,
    automation_folders: &[String],
    completion: llm_client::ChatReply,
    allow_actions: bool,
    workflow_step: bool,
) -> Result<ChatReply, String> {
    let mut response = completion.response;
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| "Unexpected API response.".to_string())?;
    if allow_actions
        && message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map_or(true, Vec::is_empty)
    {
        if let Some((tool_call, clean_content)) = compatibility_action_call(message)? {
            response["choices"][0]["message"]["tool_calls"] = json!([tool_call]);
            response["choices"][0]["message"]["content"] = json!(clean_content);
        }
    }
    let Some(message) = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
    else {
        return Err("Unexpected API response.".into());
    };
    let used_model = completion.model;
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str) {
        return Err(refusal.to_string());
    }
    if !allow_actions
        && message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|tool_calls| !tool_calls.is_empty())
    {
        let text = "Unclear: workflow planning and verification phases must return text, not an action.";
        chat.push(
            conversation_id,
            json!({ "role": "assistant", "content": text }),
        );
        return Ok(ChatReply {
            text: text.into(),
            model: used_model,
            action: None,
            provider: completion.provider,
            fallback_notice: completion.fallback_notice,
        });
    }
    if let Some(tool_calls) = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .filter(|tool_calls| allow_actions && !tool_calls.is_empty())
    {
        let mut actions = VecDeque::new();
        let mut memory_updates = Vec::new();
        let mut results = Vec::new();
        let calls_to_process = if workflow_step {
            tool_calls.iter().take(1).collect::<Vec<_>>()
        } else {
            tool_calls.iter().collect::<Vec<_>>()
        };
        for (call_index, tool_call) in calls_to_process.iter().enumerate() {
            let tool_call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "The assistant returned an invalid local action.".to_string())?
                .to_string();
            let function = tool_call
                .get("function")
                .ok_or_else(|| "The assistant returned an invalid local action.".to_string())?;
            let tool_name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "The assistant returned an invalid tool request.".to_string())?;
            if workflow_step && !is_workflow_action(tool_name) {
                return Err(
                    "Saved workflows may only run one visible desktop action at a time.".into(),
                );
            }
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| "The assistant returned invalid action arguments.".to_string())?;
            let arguments: Value = serde_json::from_str(arguments).map_err(|error| {
                format!("The assistant returned invalid action arguments: {error}")
            })?;
            if tool_name == "remember_user_fact" {
                let category = arguments
                    .get("category")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "The assistant returned a memory without a category.".to_string()
                    })?
                    .to_string();
                let fact = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "The assistant returned a memory without text.".to_string())?
                    .to_string();
                let replace_id = arguments
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                memory_updates.push((category, fact, replace_id));
                results.push(QueuedActionResult {
                    call_index,
                    tool_call_id,
                    content: "The memory was saved.".into(),
                });
                continue;
            }
            let name = automation::supports(tool_name)
                .then(|| tool_name.to_string())
                .ok_or_else(|| {
                    "The assistant requested an unsupported local action.".to_string()
                })?;
            let action = automation::prepare(
                automation::AutomationAction {
                    name,
                    arguments,
                    preview: None,
                    preview_hash: None,
                },
                automation_folders,
            )?;
            actions.push_back(QueuedAction {
                call_index,
                tool_call_id,
                action,
            });
        }
        for (category, fact, replace_id) in memory_updates {
            crate::memory::remember(&category, &fact, replace_id.as_deref())?;
        }
        if actions.is_empty() {
            let text = response_text(message);
            let text = if text.is_empty() {
                "I’ll keep that in mind.".to_string()
            } else {
                text
            };
            chat.push(
                conversation_id,
                json!({ "role": "assistant", "content": text }),
            );
            return Ok(ChatReply {
                text,
                model: used_model,
                action: None,
                provider: completion.provider,
                fallback_notice: completion.fallback_notice,
            });
        }
        let mut assistant_message = message.clone();
        if workflow_step {
            assistant_message["tool_calls"] = json!([tool_calls[0]]);
        }
        chat.push(conversation_id, assistant_message);
        let first = actions
            .pop_front()
            .ok_or_else(|| "The assistant did not return a usable local action.".to_string())?;
        chat.pending_actions.lock().unwrap().insert(
            conversation_id.to_string(),
            PendingAction {
                tool_call_id: Some(first.tool_call_id),
                call_index: first.call_index,
                action: first.action.clone(),
                remaining: actions,
                results,
                workflow_step,
            },
        );
        return Ok(ChatReply {
            text: message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
            model: used_model,
            action: Some(first.action),
            provider: completion.provider,
            fallback_notice: completion.fallback_notice,
        });
    }
    let text = response_text(message);

    if text.is_empty() {
        return Err("No response text.".into());
    }
    chat.push(
        conversation_id,
        json!({ "role": "assistant", "content": text }),
    );
    Ok(ChatReply {
        text,
        model: used_model,
        action: None,
        provider: completion.provider,
        fallback_notice: completion.fallback_notice,
    })
}

fn compatibility_action_call(message: &Value) -> Result<Option<(Value, String)>, String> {
    const OPEN: &str = "<macha_action>";
    const CLOSE: &str = "</macha_action>";
    let Some(content) = message.get("content").and_then(value_text) else {
        return Ok(None);
    };
    let Some(start) = content.find(OPEN) else {
        return Ok(None);
    };
    let json_start = start + OPEN.len();
    let Some(relative_end) = content[json_start..].find(CLOSE) else {
        return Err("The model returned an incomplete local action request.".into());
    };
    let end = json_start + relative_end;
    let action: Value = serde_json::from_str(content[json_start..end].trim())
        .map_err(|error| format!("The model returned an invalid local action request: {error}"))?;
    let name = action
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| *name == "remember_user_fact" || automation::supports(name))
        .ok_or_else(|| "The model requested an unsupported local action.".to_string())?;
    let arguments = action
        .get("arguments")
        .filter(|arguments| arguments.is_object())
        .ok_or_else(|| "The model returned invalid local action arguments.".to_string())?;
    let clean_content = format!("{}{}", &content[..start], &content[end + CLOSE.len()..])
        .trim()
        .to_string();
    Ok(Some((
        json!({
            "id": "macha-compatibility-action",
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(arguments)
                    .map_err(|error| format!("Could not parse local action arguments: {error}"))?
            }
        }),
        clean_content,
    )))
}

fn response_text(message: &Value) -> String {
    let content = message
        .get("content")
        .and_then(value_text)
        .unwrap_or_default();
    if !content.trim().is_empty() {
        return content.trim().to_string();
    }
    message
        .get("reasoning")
        .or_else(|| message.get("reasoning_content"))
        .and_then(value_text)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn value_text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string).or_else(|| {
        value.as_array().map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
    })
}

fn append_context(
    content: &mut Vec<Value>,
    context: Option<&ChatContext>,
    shared_context: &[SharedConversationContext],
) -> Result<(), String> {
    for conversation in shared_context {
        content.push(json!({
            "type": "text",
            "text": format!(
                "Background from a previous conversation titled {:?}. Use it only when relevant.",
                conversation.title
            ),
        }));
        if let Some(file) = &conversation.file {
            match file_block(&file.path) {
                Ok(block) => content.push(block),
                Err(error) => content.push(json!({
                    "type": "text",
                    "text": format!("Previous attachment {:?} is unavailable: {error}", file.name),
                })),
            }
        }
        for message in &conversation.messages {
            content.push(json!({
                "type": "text",
                "text": format!(
                    "{}: {}",
                    if message.role == "user" { "User" } else { "Assistant" },
                    message.content
                ),
            }));
        }
    }
    match context {
        Some(ChatContext::File { name, path }) => {
            content.push(file_block(path)?);
            content.push(json!({ "type": "text", "text": format!("File: {name}") }));
        }
        Some(ChatContext::Window {
            app_name,
            title,
            url,
        }) => {
            let mut text = format!("Context — App: {app_name}, Window: {title}");
            if let Some(url) = url {
                text.push_str(&format!(", URL: {url}"));
            }
            content.push(json!({ "type": "text", "text": text }));
        }
        None => {}
    }
    Ok(())
}

fn automation_tools() -> Value {
    let mut tools = json!([
        {
            "type": "function",
            "function": {
                "name": "list_directory",
                "description": "List names and types in any directory the current user can access. Requires explicit user approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Absolute path anywhere on the computer, or a path relative to a preferred folder or the user's home folder." } },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a small UTF-8 text file anywhere the current user can access. The contents are sent to the configured AI provider. Requires approval in ordinary chat; workflow actions are pre-authorized when the user starts the workflow.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "open_path",
                "description": "Open any file or folder the current user can access in the system file manager. Requires explicit user approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_directory",
                "description": "Create one new folder anywhere the current user can write. Does not create parents or overwrite. Requires approval in ordinary chat; workflow actions are pre-authorized when the user starts the workflow.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_file",
                "description": "Create a new UTF-8 text file anywhere the current user can write. Fails if the file already exists. Requires approval in ordinary chat; workflow actions are pre-authorized when the user starts the workflow.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Replace all contents of an existing UTF-8 text file anywhere the current user can write. A backup is created. Requires approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "erase_file_content",
                "description": "Clear all contents of an existing file anywhere the current user can write. A backup is created. Requires approval in ordinary chat; actions in a workflow the user explicitly started are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "move_file",
                "description": "Move a file to a new path on the same filesystem anywhere the current user can access. Does not overwrite existing files. Requires approval in ordinary chat; workflow actions are pre-authorized when the user starts the workflow.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "destination": { "type": "string" }
                    },
                    "required": ["path", "destination"],
                    "additionalProperties": false
                }
            }
        }
    ]);
    #[cfg(windows)]
    if let Some(tools) = tools.as_array_mut() {
        tools.extend(windows_automation_tools());
    }
    tools
}

#[cfg(windows)]
fn windows_automation_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "windows_list",
                "description": "List visible application windows and identify the active one. Read-only; requires approval in ordinary chat. Workflow actions are pre-authorized when the user starts the workflow.",
                "parameters": { "type": "object", "properties": {}, "additionalProperties": false }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_click",
                "description": "Click once or twice at pixel coordinates from the user's screen snapshot. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer", "minimum": 0 },
                        "y": { "type": "integer", "minimum": 0 },
                        "button": { "type": "string", "enum": ["left", "right", "middle"] },
                        "clicks": { "type": "integer", "enum": [1, 2] }
                    },
                    "required": ["x", "y", "button"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_type",
                "description": "Click an input at the supplied screen coordinates and type text, optionally selecting existing contents first or pressing Enter. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer", "minimum": 0 },
                        "y": { "type": "integer", "minimum": 0 },
                        "text": { "type": "string", "maxLength": 2000 },
                        "clear": { "type": "boolean" },
                        "pressEnter": { "type": "boolean" }
                    },
                    "required": ["x", "y", "text"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_move",
                "description": "Move the pointer to screen coordinates or drag from its current position. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer", "minimum": 0 },
                        "y": { "type": "integer", "minimum": 0 },
                        "drag": { "type": "boolean" }
                    },
                    "required": ["x", "y"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_scroll",
                "description": "Scroll at the pointer or at supplied screen coordinates. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer", "minimum": 0 },
                        "y": { "type": "integer", "minimum": 0 },
                        "axis": { "type": "string", "enum": ["vertical", "horizontal"] },
                        "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                        "steps": { "type": "integer", "minimum": 1, "maximum": 20 }
                    },
                    "required": ["axis", "direction"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_shortcut",
                "description": "Press a keyboard shortcut such as ctrl+c, alt+tab, enter, or escape. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": { "shortcut": { "type": "string", "maxLength": 80 } },
                    "required": ["shortcut"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_switch_window",
                "description": "Restore and activate the unique visible window whose title contains the requested text. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": { "title": { "type": "string", "maxLength": 256 } },
                    "required": ["title"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "windows_resize_window",
                "description": "Move and resize a uniquely matched visible window within the primary display. Requires approval in ordinary chat; actions in a user-started workflow are pre-authorized.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "maxLength": 256 },
                        "x": { "type": "integer", "minimum": 0 },
                        "y": { "type": "integer", "minimum": 0 },
                        "width": { "type": "integer", "minimum": 100, "maximum": 10000 },
                        "height": { "type": "integer", "minimum": 100, "maximum": 10000 }
                    },
                    "required": ["title", "x", "y", "width", "height"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "powershell_command",
                "description": "Run a PowerShell command as the current Windows user. It can access files and make system changes. Ordinary chat requires explicit approval; the user pre-authorizes actions by starting a saved workflow. Maximum runtime is 60 seconds.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "maxLength": 10000 },
                        "timeout": { "type": "integer", "minimum": 1, "maximum": 60 }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }
            }
        }),
    ]
}

/// PDF → document block, image → image block, text/code → inline text.
/// Mirrors readFileAsBlock() in ClaudeService.swift.
fn file_block(path: &str) -> Result<Value, String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let media_type = match ext.as_str() {
        "pdf" => Some(("document", "application/pdf")),
        "jpg" | "jpeg" => Some(("image", "image/jpeg")),
        "png" => Some(("image", "image/png")),
        "gif" => Some(("image", "image/gif")),
        "webp" => Some(("image", "image/webp")),
        _ => None,
    };

    if let Some((block_type, media)) = media_type {
        let bytes = std::fs::read(path).map_err(|e| format!("Cannot read attachment: {e}"))?;
        let data_url = format!("data:{media};base64,{}", base64(&bytes));
        return Ok(if block_type == "image" {
            json!({
                "type": "image_url",
                "image_url": { "url": data_url },
            })
        } else {
            json!({
                "type": "file",
                "file": {
                    "filename": std::path::Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("attachment.pdf"),
                    "file_data": data_url,
                },
            })
        });
    }

    let len = std::fs::metadata(path)
        .map_err(|e| format!("Cannot read attachment: {e}"))?
        .len();
    if len > MAX_INLINE_TEXT {
        return Err(format!(
            "This text file is too large to attach ({} KB maximum).",
            MAX_INLINE_TEXT / 1024
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|_| "This file is not a supported text, PDF, or image file.".to_string())?;
    Ok(json!({ "type": "text", "text": format!("File contents:\n{text}") }))
}

/// Small standalone base64 encoder — not worth another dependency.
/// Also used for Stripe's basic auth.
pub(crate) fn base64_for(bytes: &[u8]) -> String {
    base64(bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        append_context, base64, file_block, handle_response, handle_response_mode,
        local_file_action_intent, provider_messages, response_text, Chat, ChatContext,
        ChatHistoryMessage, SharedContextFile, SharedConversationContext,
    };
    use super::{assistant_tools, tool_support_error, validate_screen_image, with_screen_image};
    use crate::llm_client::ChatReply as LlmReply;
    use serde_json::Value;
    use std::fs;

    #[test]
    fn tool_call_is_returned_as_a_pending_user_approval() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "model": "test/model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "list_directory",
                            "arguments": "{\"path\":\"notes\"}"
                        }
                    }]
                }
            }]
        });
        let folder = std::env::temp_dir().join(format!("macha-tool-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("notes")).unwrap();
        let folders = vec![folder.to_string_lossy().into_owned()];
        let reply = handle_response(
            &chat,
            "conversation",
            &folders,
            LlmReply {
                response,
                provider: "ollama".into(),
                model: "qwen2.5:7b".into(),
                fallback_notice: None,
            },
        )
        .unwrap();
        assert_eq!(reply.action.as_ref().unwrap().name, "list_directory");
        assert!(chat
            .pending_actions
            .lock()
            .unwrap()
            .contains_key("conversation"));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn empty_tool_call_array_does_not_hide_a_normal_assistant_reply() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello, how can I help?",
                    "tool_calls": []
                }
            }]
        });
        assert!(!super::has_empty_action_response(&response));

        let reply = handle_response(
            &chat,
            "empty-tool-calls-with-text",
            &[],
            LlmReply {
                response,
                provider: "openrouter".into(),
                model: "test/model".into(),
                fallback_notice: None,
            },
        )
        .unwrap();

        assert_eq!(reply.text, "Hello, how can I help?");
        assert!(reply.action.is_none());
    }

    #[test]
    fn empty_tool_call_array_without_text_is_detected_for_compatibility_retry() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": []
                }
            }]
        });
        assert!(super::has_empty_action_response(&response));
    }

    #[test]
    fn multiple_tool_calls_are_queued_for_separate_approvals() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call-first",
                            "type": "function",
                            "function": {
                                "name": "open_website",
                                "arguments": "{\"url\":\"https://example.com\"}"
                            }
                        },
                        {
                            "id": "call-second",
                            "type": "function",
                            "function": {
                                "name": "search_web",
                                "arguments": "{\"query\":\"Macha assistant\"}"
                            }
                        }
                    ]
                }
            }]
        });
        let reply = handle_response(
            &chat,
            "multi-action-conversation",
            &[],
            LlmReply {
                response,
                provider: "ollama".into(),
                model: "qwen2.5:7b".into(),
                fallback_notice: None,
            },
        )
        .unwrap();

        assert_eq!(reply.action.as_ref().unwrap().name, "open_website");
        let pending_actions = chat.pending_actions.lock().unwrap();
        let pending = pending_actions.get("multi-action-conversation").unwrap();
        assert_eq!(pending.tool_call_id.as_deref(), Some("call-first"));
        assert_eq!(pending.remaining.len(), 1);
        assert_eq!(pending.remaining[0].tool_call_id, "call-second");
        assert_eq!(pending.remaining[0].action.name, "open_website");
    }

    #[test]
    fn batches_over_five_actions_are_queued_one_at_a_time() {
        let chat = Chat::default();
        let tool_calls: Vec<Value> = (0..6)
            .map(|index| {
                serde_json::json!({
                    "id": format!("call-{index}"),
                    "type": "function",
                    "function": {
                        "name": "open_website",
                        "arguments": format!("{{\"url\":\"https://example.com/{index}\"}}")
                    }
                })
            })
            .collect();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tool_calls
                }
            }]
        });

        let reply = handle_response(
            &chat,
            "large-action-batch",
            &[],
            LlmReply {
                response,
                provider: "openrouter".into(),
                model: "test/model".into(),
                fallback_notice: None,
            },
        )
        .unwrap();

        assert_eq!(reply.action.as_ref().unwrap().name, "open_website");
        let pending_actions = chat.pending_actions.lock().unwrap();
        let pending = pending_actions.get("large-action-batch").unwrap();
        assert_eq!(pending.tool_call_id.as_deref(), Some("call-0"));
        assert_eq!(pending.remaining.len(), 5);
        assert_eq!(pending.remaining[0].tool_call_id, "call-1");
    }

    #[cfg(windows)]
    #[test]
    fn workflow_response_keeps_only_the_first_action() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "workflow-first",
                            "type": "function",
                            "function": {
                                "name": "windows_click",
                                "arguments": "{\"x\":120,\"y\":240,\"button\":\"left\"}"
                            }
                        },
                        {
                            "id": "workflow-discarded",
                            "type": "function",
                            "function": {
                                "name": "windows_shortcut",
                                "arguments": "{\"shortcut\":\"enter\"}"
                            }
                        }
                    ]
                }
            }]
        });
        let reply = handle_response_mode(
            &chat,
            "workflow-single-action",
            &[],
            LlmReply {
                response,
                provider: "ollama".into(),
                model: "qwen2.5:7b".into(),
                fallback_notice: None,
            },
            true,
            true,
        )
        .unwrap();

        assert_eq!(reply.action.as_ref().unwrap().name, "windows_click");
        let pending_actions = chat.pending_actions.lock().unwrap();
        let pending = pending_actions.get("workflow-single-action").unwrap();
        assert!(pending.remaining.is_empty());
        assert!(pending.workflow_step);
        let history = chat.snapshot("workflow-single-action");
        let assistant = history
            .iter()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            .unwrap();
        assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn workflow_plan_and_verification_responses_cannot_queue_actions() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "not-allowed",
                        "type": "function",
                        "function": {
                            "name": "open_website",
                            "arguments": "{\"url\":\"https://example.com\"}"
                        }
                    }]
                }
            }]
        });
        let reply = handle_response_mode(
            &chat,
            "workflow-read-only-phase",
            &[],
            LlmReply {
                response,
                provider: "ollama".into(),
                model: "qwen2.5:7b".into(),
                fallback_notice: None,
            },
            false,
            false,
        )
        .unwrap();
        assert!(reply.action.is_none());
        assert!(reply.text.contains("must return text"));
        assert!(!chat
            .pending_actions
            .lock()
            .unwrap()
            .contains_key("workflow-read-only-phase"));
    }

    #[test]
    fn invalid_tool_call_in_batch_does_not_queue_any_action() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call-valid",
                            "type": "function",
                            "function": {
                                "name": "open_website",
                                "arguments": "{\"url\":\"https://example.com\"}"
                            }
                        },
                        {
                            "id": "call-invalid",
                            "type": "function",
                            "function": {
                                "name": "run_shell",
                                "arguments": "{}"
                            }
                        }
                    ]
                }
            }]
        });
        assert!(handle_response(
            &chat,
            "invalid-batch-conversation",
            &[],
            LlmReply {
                response,
                provider: "ollama".into(),
                model: "qwen2.5:7b".into(),
                fallback_notice: None,
            },
        )
        .is_err());
        assert!(!chat
            .pending_actions
            .lock()
            .unwrap()
            .contains_key("invalid-batch-conversation"));
    }

    #[test]
    fn compatibility_action_is_validated_and_waits_for_approval() {
        let chat = Chat::default();
        let folder = std::env::temp_dir().join(format!(
            "macha-compatibility-action-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("created.txt");
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "I can create that.\n<macha_action>{\"name\":\"create_file\",\"arguments\":{\"path\":\"created.txt\",\"content\":\"hello\"}}</macha_action>"
                }
            }]
        });
        let folders = vec![folder.to_string_lossy().into_owned()];
        let reply = handle_response(
            &chat,
            "compatibility-conversation",
            &folders,
            LlmReply {
                response,
                provider: "openrouter".into(),
                model: "test/model".into(),
                fallback_notice: None,
            },
        )
        .unwrap();

        assert_eq!(reply.action.as_ref().unwrap().name, "create_file");
        assert_eq!(reply.text, "I can create that.");
        assert!(!path.exists());
        assert!(chat
            .pending_actions
            .lock()
            .unwrap()
            .contains_key("compatibility-conversation"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn powershell_command_is_queued_without_running_until_approved() {
        let chat = Chat::default();
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "powershell-call",
                        "type": "function",
                        "function": {
                            "name": "powershell_command",
                            "arguments": "{\"command\":\"Get-Date\"}"
                        }
                    }]
                }
            }]
        });
        let reply = handle_response(
            &chat,
            "powershell-approval",
            &[],
            LlmReply {
                response,
                provider: "ollama".into(),
                model: "qwen2.5:7b".into(),
                fallback_notice: None,
            },
        )
        .unwrap();

        assert_eq!(reply.action.as_ref().unwrap().name, "powershell_command");
        assert_eq!(
            reply.action.as_ref().unwrap().arguments["command"],
            "Get-Date"
        );
        assert!(chat
            .pending_actions
            .lock()
            .unwrap()
            .contains_key("powershell-approval"));
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn file_block_reads_text_and_encodes_images() {
        let dir = std::env::temp_dir().join(format!("macha-chat-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let text_path = dir.join("note.txt");
        fs::write(&text_path, "hello").unwrap();
        let text = file_block(text_path.to_str().unwrap()).unwrap();
        assert_eq!(text["type"], "text");
        assert_eq!(text["text"], "File contents:\nhello");

        let image_path = dir.join("pixel.png");
        fs::write(&image_path, b"png bytes").unwrap();
        let image = file_block(image_path.to_str().unwrap()).unwrap();
        assert_eq!(image["type"], "image_url");
        assert_eq!(
            image["image_url"]["url"],
            format!("data:image/png;base64,{}", base64(b"png bytes"))
        );

        let pdf_path = dir.join("document.pdf");
        fs::write(&pdf_path, b"%PDF test").unwrap();
        let pdf = file_block(pdf_path.to_str().unwrap()).unwrap();
        assert_eq!(pdf["type"], "file");
        assert_eq!(pdf["file"]["filename"], "document.pdf");
        assert_eq!(
            pdf["file"]["file_data"],
            format!("data:application/pdf;base64,{}", base64(b"%PDF test"))
        );

        let binary_path = dir.join("document.docx");
        fs::write(&binary_path, [0xff, 0xfe]).unwrap();
        assert!(file_block(binary_path.to_str().unwrap())
            .unwrap_err()
            .contains("not a supported text"));

        let missing_path = dir.join("missing.txt");
        assert!(file_block(missing_path.to_str().unwrap())
            .unwrap_err()
            .contains("Cannot read attachment"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conversation_histories_are_isolated_and_can_be_restored() {
        let chat = Chat::default();
        let history = vec![
            ChatHistoryMessage {
                role: "user".into(),
                content: "Summarize this".into(),
            },
            ChatHistoryMessage {
                role: "assistant".into(),
                content: "A short summary".into(),
            },
        ];

        chat.seed_history("first", &history, None, &[]).unwrap();
        chat.seed_history("second", &[], None, &[]).unwrap();

        assert_eq!(chat.snapshot("first").len(), 2);
        assert!(chat.snapshot("second").is_empty());

        chat.seed_history("first", &[], None, &[]).unwrap();
        assert_eq!(chat.snapshot("first").len(), 2);
        assert!(chat
            .seed_history(
                "invalid",
                &[ChatHistoryMessage {
                    role: "system".into(),
                    content: "no".into()
                }],
                None,
                &[],
            )
            .is_err());
    }

    #[test]
    fn deleting_conversation_clears_only_its_model_history() {
        let chat = Chat::default();
        chat.push(
            "first",
            serde_json::json!({ "role": "user", "content": [] }),
        );
        chat.push(
            "second",
            serde_json::json!({ "role": "user", "content": [] }),
        );

        chat.delete("first");

        assert!(chat.snapshot("first").is_empty());
        assert_eq!(chat.snapshot("second").len(), 1);
    }

    #[test]
    fn restored_file_conversation_reloads_its_attachment_context() {
        let dir = std::env::temp_dir().join(format!("macha-restore-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.txt");
        fs::write(&path, "private local file contents").unwrap();

        let chat = Chat::default();
        let history = [ChatHistoryMessage {
            role: "user".into(),
            content: "What is this?".into(),
        }];
        chat.seed_history(
            "file-conversation",
            &history,
            Some(&ChatContext::File {
                name: "note.txt".into(),
                path: path.to_string_lossy().into_owned(),
            }),
            &[SharedConversationContext {
                title: "earlier".into(),
                messages: vec![ChatHistoryMessage {
                    role: "user".into(),
                    content: "remember this".into(),
                }],
                file: None,
            }],
        )
        .unwrap();
        let seeded = chat.snapshot("file-conversation");
        assert!(seeded[0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("earlier"));
        assert_eq!(seeded[0]["content"][1]["text"], "User: remember this");
        assert_eq!(
            seeded[0]["content"][2]["text"],
            "File contents:\nprivate local file contents"
        );
        assert_eq!(seeded[0]["content"][3]["text"], "File: note.txt");
        assert_eq!(seeded[0]["content"][4]["text"], "What is this?");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shared_context_includes_previous_messages_and_file_contents() {
        let dir = std::env::temp_dir().join(format!("macha-shared-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reference.txt");
        fs::write(&path, "shared attachment facts").unwrap();

        let previous = [SharedConversationContext {
            title: "Reference chat".into(),
            messages: vec![
                ChatHistoryMessage {
                    role: "user".into(),
                    content: "Remember the project name".into(),
                },
                ChatHistoryMessage {
                    role: "assistant".into(),
                    content: "The project is Macha".into(),
                },
            ],
            file: Some(SharedContextFile {
                name: "reference.txt".into(),
                path: path.to_string_lossy().into_owned(),
            }),
        }];
        let mut blocks = Vec::new();
        append_context(&mut blocks, None, &previous).unwrap();

        assert!(blocks
            .iter()
            .any(|block| block["text"] == "User: Remember the project name"));
        assert!(blocks
            .iter()
            .any(|block| block["text"] == "Assistant: The project is Macha"));
        assert!(blocks
            .iter()
            .any(|block| block["text"] == "File contents:\nshared attachment facts"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn final_answer_content_takes_precedence_over_reasoning() {
        let message = serde_json::json!({
            "content": "Final answer",
            "reasoning": "private reasoning",
            "reasoning_content": "also private",
        });
        assert_eq!(response_text(&message), "Final answer");
    }

    #[test]
    fn reasoning_is_used_only_when_no_final_content_exists() {
        let message = serde_json::json!({
            "content": "",
            "reasoning_content": "fallback content",
        });
        assert_eq!(response_text(&message), "fallback content");
    }

    #[test]
    fn provider_history_keeps_system_prompt_and_only_recent_twenty_messages() {
        let history = (0..25)
            .map(|index| {
                serde_json::json!({
                    "role": if index % 2 == 0 { "user" } else { "assistant" },
                    "content": index.to_string(),
                })
            })
            .collect();
        let messages = provider_messages(history, &[], &[], false);
        assert_eq!(messages.len(), 21);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["content"], "5");
        assert_eq!(messages.last().unwrap()["content"], "24");
    }

    #[test]
    fn screen_images_are_added_to_the_current_request_without_mutating_history() {
        let history = vec![serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "What is on my screen?" }],
        })];
        let request = with_screen_image(history.clone(), Some("data:image/jpeg;base64,AAAA"));
        assert_eq!(history[0]["content"].as_array().unwrap().len(), 1);
        assert_eq!(request[0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(request[0]["content"][1]["type"], "image_url");
        assert!(validate_screen_image("data:image/jpeg;base64,AAAA").is_ok());
        assert!(validate_screen_image("https://example.com/image.jpg").is_err());
    }

    #[test]
    fn all_local_tools_are_available_without_selected_folders() {
        let tools = assistant_tools();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert!(names.contains(&"remember_user_fact"));
        assert!(names.contains(&"open_app"));
        assert!(names.contains(&"open_website"));
        assert!(names.contains(&"search_web"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"list_directory"));
        assert!(names.contains(&"write_file"));
        for name in [
            "windows_list",
            "windows_click",
            "windows_type",
            "windows_move",
            "windows_scroll",
            "windows_shortcut",
            "windows_switch_window",
            "windows_resize_window",
            "powershell_command",
        ] {
            assert_eq!(
                names.contains(&name),
                cfg!(windows),
                "unexpected tool {name}"
            );
        }
        if cfg!(windows) {
            let powershell = tools
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["function"]["name"] == "powershell_command")
                .unwrap();
            assert!(powershell["function"]["description"]
                .as_str()
                .unwrap()
                .contains("explicit approval"));
        }
    }

    #[test]
    fn workflow_tools_are_limited_to_single_visible_actions() {
        let tools = super::workflow_action_tools();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
            })
            .collect();
        assert!(names.contains(&"open_app"));
        assert!(names.iter().all(|name| super::is_workflow_action(name)));
        assert!(!names.contains(&"search_web"));
        assert!(!names.contains(&"powershell_command"));
        assert!(!names.contains(&"remember_user_fact"));
    }

    #[test]
    fn unsupported_tool_models_can_retry_with_action_compatibility() {
        assert!(tool_support_error(
            "This model does not support function calls."
        ));
        assert!(tool_support_error("This provider doesn't support tools."));
        assert!(tool_support_error(
            "No endpoints found that support tool use."
        ));
        assert!(!tool_support_error("The request timed out."));
    }

    #[test]
    fn local_file_action_intent_selects_the_model_agnostic_action_path() {
        assert!(local_file_action_intent(
            "Please create a file named notes.txt"
        ));
        assert!(local_file_action_intent("Can you list this folder?"));
        assert!(local_file_action_intent(
            "move report.txt to C:\\Users\\me\\Docs"
        ));
        assert!(!local_file_action_intent(
            "How do I create a file in this folder?"
        ));
        assert!(!local_file_action_intent("Explain how to edit a document"));
        assert!(!local_file_action_intent("What files can Macha access?"));
    }

    #[test]
    fn compatibility_action_envelope_becomes_a_validated_tool_request() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": "I can do that.\n<macha_action>{\"name\":\"open_app\",\"arguments\":{\"appName\":\"Google Chrome\"}}</macha_action>"
        });
        let (call, clean_content) = super::compatibility_action_call(&message).unwrap().unwrap();
        assert_eq!(call["function"]["name"], "open_app");
        assert_eq!(
            serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap()
                ["appName"],
            "Google Chrome"
        );
        assert_eq!(clean_content, "I can do that.");
    }

    #[test]
    fn compatibility_action_envelope_rejects_unsupported_and_malformed_requests() {
        let unsupported = serde_json::json!({
            "content": "<macha_action>{\"name\":\"run_shell\",\"arguments\":{}}</macha_action>"
        });
        assert!(super::compatibility_action_call(&unsupported).is_err());
        let malformed = serde_json::json!({
            "content": "<macha_action>{not json}</macha_action>"
        });
        assert!(super::compatibility_action_call(&malformed).is_err());
        let incomplete = serde_json::json!({
            "content": "<macha_action>{\"name\":\"open_app\"}"
        });
        assert!(super::compatibility_action_call(&incomplete).is_err());
    }

    #[test]
    fn compatibility_prompt_removes_native_tool_history() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "system"}),
            serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "call-1"}]
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-1",
                "content": "Created file."
            }),
            serde_json::json!({"role": "user", "content": "Thanks"}),
        ];
        let adapted = super::compatibility_messages(messages).unwrap();
        assert_eq!(adapted.len(), 3);
        assert!(adapted[0]["content"]
            .as_str()
            .unwrap()
            .contains("<macha_action>"));
        assert_eq!(adapted[1]["role"], "assistant");
        assert_eq!(adapted[1]["content"], "Macha action result: Created file.");
        assert_eq!(adapted[2]["content"], "Thanks");
    }
}
