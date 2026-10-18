use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const MAX_WORKFLOWS: usize = 500;
const MAX_NAME_CHARS: usize = 100;
const MAX_PROMPT_CHARS: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub archived: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDraft {
    pub id: Option<String>,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRun {
    id: String,
    name: String,
    prompt: String,
}

fn path() -> PathBuf {
    crate::settings::config_dir().join("workflows.json")
}

fn validate_draft(name: &str, prompt: &str) -> Result<(String, String), String> {
    let name = name.trim();
    let prompt = prompt.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "Workflow names must be between 1 and {MAX_NAME_CHARS} characters."
        ));
    }
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "Workflow instructions must be between 1 and {MAX_PROMPT_CHARS} characters."
        ));
    }
    Ok((name.to_string(), prompt.to_string()))
}

fn load_from(path: &std::path::Path) -> Result<Vec<Workflow>, String> {
    match fs::read(path) {
        Ok(bytes) => decode(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read saved workflows: {error}")),
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<Workflow>, String> {
    let workflows: Vec<Workflow> = serde_json::from_slice(bytes)
        .map_err(|error| format!("Saved workflows are invalid: {error}"))?;
    if workflows.len() > MAX_WORKFLOWS {
        return Err(format!(
            "Saved workflows exceed the limit of {MAX_WORKFLOWS}."
        ));
    }
    Ok(workflows)
}

fn save_to(path: &std::path::Path, workflows: &[Workflow]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The workflow storage path is invalid.".to_string())?;
    crate::platform::ensure_private_dir(parent)
        .map_err(|error| format!("Could not prepare workflow storage: {error}"))?;
    let bytes = serde_json::to_vec_pretty(workflows)
        .map_err(|error| format!("Could not encode workflows: {error}"))?;
    fs::write(path, bytes).map_err(|error| format!("Could not save workflows: {error}"))
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|error| format!("System clock is before the Unix epoch: {error}"))
}

fn new_id() -> String {
    let mut bytes = [0; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn list() -> Result<Vec<Workflow>, String> {
    load_from(&path())
}

pub fn save(draft: WorkflowDraft) -> Result<Workflow, String> {
    save_at(&path(), draft)
}

fn save_at(path: &std::path::Path, draft: WorkflowDraft) -> Result<Workflow, String> {
    let (name, prompt) = validate_draft(&draft.name, &draft.prompt)?;
    let mut workflows = load_from(path)?;
    if let Some(id) = draft.id {
        let workflow = workflows
            .iter_mut()
            .find(|workflow| workflow.id == id)
            .ok_or_else(|| "That workflow no longer exists.".to_string())?;
        workflow.name = name;
        workflow.prompt = prompt;
        workflow.updated_at = now()?;
        let saved = workflow.clone();
        save_to(path, &workflows)?;
        Ok(saved)
    } else {
        if workflows.len() >= MAX_WORKFLOWS {
            return Err(format!("You can save up to {MAX_WORKFLOWS} workflows."));
        }
        let timestamp = now()?;
        let workflow = Workflow {
            id: new_id(),
            name,
            prompt,
            archived: false,
            created_at: timestamp,
            updated_at: timestamp,
        };
        workflows.push(workflow.clone());
        save_to(path, &workflows)?;
        Ok(workflow)
    }
}

pub fn set_archived(id: &str, archived: bool) -> Result<(), String> {
    set_archived_at(&path(), id, archived)
}

fn set_archived_at(path: &std::path::Path, id: &str, archived: bool) -> Result<(), String> {
    let mut workflows = load_from(path)?;
    let workflow = workflows
        .iter_mut()
        .find(|workflow| workflow.id == id)
        .ok_or_else(|| "That workflow no longer exists.".to_string())?;
    workflow.archived = archived;
    workflow.updated_at = now()?;
    save_to(path, &workflows)
}

pub fn delete(id: &str) -> Result<(), String> {
    delete_at(&path(), id)
}

fn delete_at(path: &std::path::Path, id: &str) -> Result<(), String> {
    let mut workflows = load_from(path)?;
    let original_len = workflows.len();
    workflows.retain(|workflow| workflow.id != id);
    if workflows.len() == original_len {
        return Err("That workflow no longer exists.".into());
    }
    save_to(path, &workflows)
}

pub fn run(app: &AppHandle, id: &str) -> Result<(), String> {
    let workflows = load_from(&path())?;
    let workflow = workflows
        .iter()
        .find(|workflow| workflow.id == id)
        .ok_or_else(|| "That workflow no longer exists.".to_string())?;
    if workflow.archived {
        return Err("Unarchive this workflow before running it.".into());
    }
    let payload = WorkflowRun {
        id: workflow.id.clone(),
        name: workflow.name.clone(),
        prompt: workflow.prompt.clone(),
    };
    let _ = app.emit_to(crate::island::WINDOW_LABEL, "workflow-run", payload.clone());
    app.emit("workflow-run", payload)
        .map_err(|error| format!("Could not start the workflow: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        decode, delete_at, load_from, save_at, set_archived_at, validate_draft, Workflow,
        WorkflowDraft,
    };
    use std::path::PathBuf;

    fn test_file() -> PathBuf {
        std::env::temp_dir()
            .join(format!("coucou-workflows-test-{}", super::new_id()))
            .join("workflows.json")
    }

    #[test]
    fn validates_and_trims_workflow_drafts() {
        assert_eq!(
            validate_draft("  Daily review  ", "  Check my project  ").unwrap(),
            ("Daily review".into(), "Check my project".into())
        );
        assert!(validate_draft("  ", "prompt").is_err());
        assert!(validate_draft("name", "  ").is_err());
        assert!(validate_draft("x".repeat(101).as_str(), "prompt").is_err());
        assert!(validate_draft("name", "x".repeat(10_001).as_str()).is_err());
    }

    #[test]
    fn workflow_records_round_trip_with_camel_case_fields() {
        let workflow = Workflow {
            id: "a1".into(),
            name: "Daily review".into(),
            prompt: "Review my project".into(),
            archived: false,
            created_at: 10,
            updated_at: 11,
        };
        let encoded = serde_json::to_vec(&vec![workflow.clone()]).unwrap();
        assert!(String::from_utf8_lossy(&encoded).contains("\"createdAt\""));
        assert_eq!(
            serde_json::from_slice::<Vec<Workflow>>(&encoded).unwrap(),
            vec![workflow]
        );
    }

    #[test]
    fn missing_workflow_file_means_no_saved_workflows() {
        let path = test_file();
        assert!(load_from(&path).unwrap().is_empty());
    }

    #[test]
    fn malformed_workflow_file_is_reported() {
        assert!(decode(b"{")
            .unwrap_err()
            .starts_with("Saved workflows are invalid:"));
    }

    #[test]
    fn workflows_can_be_created_updated_archived_restored_and_deleted() {
        let path = test_file();
        let created = save_at(
            &path,
            WorkflowDraft {
                id: None,
                name: "Morning review".into(),
                prompt: "Review today's schedule".into(),
            },
        )
        .unwrap();
        assert_eq!(load_from(&path).unwrap(), vec![created.clone()]);

        let updated = save_at(
            &path,
            WorkflowDraft {
                id: Some(created.id.clone()),
                name: "Daily review".into(),
                prompt: "Review my schedule and inbox".into(),
            },
        )
        .unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.created_at, created.created_at);
        assert_eq!(updated.name, "Daily review");

        set_archived_at(&path, &updated.id, true).unwrap();
        assert!(load_from(&path).unwrap()[0].archived);
        set_archived_at(&path, &updated.id, false).unwrap();
        assert!(!load_from(&path).unwrap()[0].archived);

        delete_at(&path, &updated.id).unwrap();
        assert!(load_from(&path).unwrap().is_empty());
        assert!(delete_at(&path, &updated.id).is_err());

        let parent = path.parent().unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }
}
