use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use tauri::{AppHandle, Manager, State};
use tokio::sync::{Mutex, RwLock};

use super::{
    AgentPlanState, AgentRuntimeConfig, AgentTodoState, ChatAssistantSnapshot, ChatMessage,
    Conversation, ConversationContextState, ConversationListItem, ModelRef, WebSearchMode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationRepositoryError {
    Conflict {
        id: String,
        expected: u64,
        actual: u64,
    },
    Storage(String),
}

impl fmt::Display for ConversationRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict {
                id,
                expected,
                actual,
            } => write!(
                f,
                "conversation revision conflict ({id}): expected {expected}, actual {actual}"
            ),
            Self::Storage(message) => f.write_str(message),
        }
    }
}

impl From<String> for ConversationRepositoryError {
    fn from(value: String) -> Self {
        Self::Storage(value)
    }
}

pub type RepositoryResult<T> = Result<T, ConversationRepositoryError>;

/// Explicit metadata changes. `Option` values mean set/clear, never "ignore";
/// choosing a variant is what expresses that the field must be changed.
#[derive(Debug, Clone)]
pub enum ConversationMetadataMutation {
    Title(String),
    Model {
        provider_id: String,
        model: String,
    },
    AgentRuntime(AgentRuntimeConfig),
    ActiveSkill(Option<String>),
    Assistant {
        assistant_id: Option<String>,
        assistant_snapshot: Option<ChatAssistantSnapshot>,
    },
    Pinned(bool),
    Location {
        folder: Option<String>,
        project_id: Option<String>,
        set_id: Option<String>,
    },
    KnowledgeBases {
        ids: Vec<String>,
        force_search: bool,
    },
    ThinkingLevel(Option<String>),
    WebSearchMode(Option<WebSearchMode>),
    ReplyModels(Vec<ModelRef>),
    GroupSelection {
        group_id: String,
        message_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum MessageMutation {
    Upsert(Vec<ChatMessage>),
    Replace {
        message_id: String,
        message: ChatMessage,
    },
    Remove {
        message_id: String,
    },
    TruncateFrom {
        message_id: String,
        include_anchor: bool,
    },
    ReplaceAll(Vec<ChatMessage>),
}

fn validate_expected_revision(
    id: &str,
    actual: u64,
    expected: Option<u64>,
) -> RepositoryResult<()> {
    if let Some(expected) = expected {
        if actual != expected {
            return Err(ConversationRepositoryError::Conflict {
                id: id.to_string(),
                expected,
                actual,
            });
        }
    }
    Ok(())
}

fn increment_revision(conversation: &mut Conversation) -> RepositoryResult<()> {
    conversation.revision = conversation.revision.checked_add(1).ok_or_else(|| {
        ConversationRepositoryError::Storage("conversation revision overflow".into())
    })?;
    Ok(())
}

fn apply_upserts(conversation: &mut Conversation, messages: Vec<ChatMessage>) {
    for message in messages {
        if let Some(position) = conversation
            .messages
            .iter()
            .position(|item| item.id == message.id)
        {
            conversation.messages[position] = message;
        } else {
            conversation.messages.push(message);
        }
    }
}

fn apply_message_mutation(
    conversation: &mut Conversation,
    mutation: MessageMutation,
) -> Result<(), String> {
    match mutation {
        MessageMutation::Upsert(messages) => apply_upserts(conversation, messages),
        MessageMutation::Replace {
            message_id,
            message,
        } => {
            let position = conversation
                .messages
                .iter()
                .position(|item| item.id == message_id)
                .ok_or_else(|| format!("message not found: {message_id}"))?;
            conversation.messages[position] = message;
        }
        MessageMutation::Remove { message_id } => {
            let position = conversation
                .messages
                .iter()
                .position(|item| item.id == message_id)
                .ok_or_else(|| format!("message not found: {message_id}"))?;
            conversation.messages.remove(position);
        }
        MessageMutation::TruncateFrom {
            message_id,
            include_anchor,
        } => {
            let position = conversation
                .messages
                .iter()
                .position(|item| item.id == message_id)
                .ok_or_else(|| format!("message not found: {message_id}"))?;
            conversation
                .messages
                .truncate(position + usize::from(!include_anchor));
        }
        MessageMutation::ReplaceAll(messages) => conversation.messages = messages,
    }
    Ok(())
}

/// Process-local equivalent of Pi's keyed operation queue. The barrier is held
/// read-only for single-session work and exclusively for list/bulk operations.
pub struct ConversationRepository {
    barrier: RwLock<()>,
    conversation_locks: StdMutex<HashMap<String, Weak<Mutex<()>>>>,
    index_lock: Mutex<()>,
}

impl Default for ConversationRepository {
    fn default() -> Self {
        Self {
            barrier: RwLock::new(()),
            conversation_locks: StdMutex::new(HashMap::new()),
            index_lock: Mutex::new(()),
        }
    }
}

impl ConversationRepository {
    fn conversation_lock(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .conversation_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(id.to_string(), Arc::downgrade(&lock));
        lock
    }

    pub async fn get(&self, app: &AppHandle, id: &str) -> RepositoryResult<Conversation> {
        let _barrier = self.barrier.read().await;
        let lock = self.conversation_lock(id);
        let _conversation = lock.lock().await;
        super::storage::load_conversation(app, id).map_err(Into::into)
    }

    /// 以下三个纯读操作只拿共享 barrier，且**不拿 `index_lock`**。
    ///
    /// `index_lock` 存在的意义是串行化 index.json 的 read-modify-write（`persist_locked` /
    /// `bulk_mutate_loaded` / `delete_conversation`）；纯读方不改任何东西，而 `atomic_write`
    /// 是 write-temp + rename，读者只会看到完整的旧版本或完整的新版本，撕不了。以前这几个
    /// 读操作既拿独占 barrier 又拿 index_lock，于是"流式回答中途在侧栏搜一下"会让 agent 每轮
    /// 的 `persist_partial_assistant` 排队等一次全量文件扫描，反之亦然。
    ///
    /// 共享 barrier 依然挡住 `bulk_mutate_loaded`（它拿独占），所以"批量迁移期间读到半套数据"
    /// 这条不变式没变。加锁顺序仍是 barrier → conversation → index，没有新的环。
    pub async fn list(
        &self,
        app: &AppHandle,
        offset: usize,
        limit: usize,
        folder: Option<String>,
        project_id: Option<String>,
        set_id: Option<String>,
    ) -> RepositoryResult<Vec<ConversationListItem>> {
        let _barrier = self.barrier.read().await;
        super::storage::get_conversations(app, offset, limit, folder, project_id, set_id)
            .map_err(Into::into)
    }

    pub async fn search(
        &self,
        app: &AppHandle,
        query: &str,
        limit: usize,
    ) -> RepositoryResult<Vec<ConversationListItem>> {
        let _barrier = self.barrier.read().await;
        super::storage::search_conversations(app, query, limit).map_err(Into::into)
    }

    /// 只读探查，不写。"同一个空对话被两个新建请求同时复用"这条不变式由调用方的
    /// `AppState::chat_create_conversation_lock` 保证（独占 barrier 从来也保证不了它：
    /// 复用一个空对话不会改动它，串行化两次探查照样都会命中同一条）。
    #[allow(clippy::too_many_arguments)]
    pub async fn find_reusable_blank(
        &self,
        app: &AppHandle,
        provider_id: &str,
        model: &str,
        folder: Option<&str>,
        project_id: Option<&str>,
        set_id: Option<&str>,
        assistant_id: Option<&str>,
    ) -> RepositoryResult<Option<Conversation>> {
        let _barrier = self.barrier.read().await;
        super::storage::find_reusable_blank_conversation(
            app,
            provider_id,
            model,
            folder,
            project_id,
            set_id,
            assistant_id,
        )
        .map_err(Into::into)
    }

    pub async fn delete(&self, app: &AppHandle, id: &str) -> RepositoryResult<Vec<String>> {
        let _barrier = self.barrier.read().await;
        let lock = self.conversation_lock(id);
        let _conversation = lock.lock().await;
        let _index = self.index_lock.lock().await;
        super::storage::delete_conversation(app, id).map_err(Into::into)
    }

    /// Exclusive multi-conversation mutation used by project/set/workspace
    /// migrations. The barrier prevents any keyed operation from interleaving;
    /// the index is written once after all affected conversation files.
    pub async fn bulk_mutate<F>(&self, app: &AppHandle, mut mutation: F) -> RepositoryResult<usize>
    where
        F: FnMut(&mut Conversation) -> Result<bool, String>,
    {
        self.bulk_mutate_loaded(app, move |conversations| {
            let mut changed = Vec::new();
            for conversation in conversations {
                if mutation(conversation)? {
                    changed.push(conversation.id.clone());
                }
            }
            Ok(changed)
        })
        .await
    }

    /// Runs a multi-conversation operation under the exclusive barrier. The
    /// callback sees a consistent set of latest files and returns the IDs that
    /// actually changed, allowing callers to preflight all filesystem work
    /// before the first persisted mutation.
    pub async fn bulk_mutate_loaded<F>(
        &self,
        app: &AppHandle,
        operation: F,
    ) -> RepositoryResult<usize>
    where
        F: FnOnce(&mut [Conversation]) -> Result<Vec<String>, String>,
    {
        let _barrier = self.barrier.write().await;
        let _index = self.index_lock.lock().await;
        let mut index = super::storage::load_index_or_scan(app)?;
        let mut conversations = Vec::with_capacity(index.conversations.len());
        for item in &index.conversations {
            let path = super::storage::conversation_file_path(app, &item.id)?;
            if !path.exists() {
                continue;
            }
            conversations.push(super::storage::load_conversation(app, &item.id)?);
        }

        let changed_ids = operation(&mut conversations)?;
        let changed_ids: std::collections::HashSet<_> = changed_ids.into_iter().collect();
        let mut changed = 0usize;
        for mut conversation in conversations {
            if !changed_ids.contains(&conversation.id) {
                continue;
            }
            increment_revision(&mut conversation)?;
            conversation.updated_at = chrono::Local::now().timestamp();
            let persisted = super::storage::write_conversation_file(app, &conversation)?;
            let item = ConversationListItem::from(&persisted);
            if let Some(position) = index
                .conversations
                .iter()
                .position(|candidate| candidate.id == persisted.id)
            {
                index.conversations[position] = item;
            } else {
                index.conversations.push(item);
            }
            changed += 1;
        }
        if changed > 0 {
            super::storage::save_index(app, &index)?;
        }
        Ok(changed)
    }

    pub async fn rewrite_artifact_paths(
        &self,
        app: &AppHandle,
        id: &str,
        mappings: Vec<(PathBuf, PathBuf)>,
    ) -> RepositoryResult<Conversation> {
        let _barrier = self.barrier.read().await;
        let lock = self.conversation_lock(id);
        let _conversation = lock.lock().await;
        let mut latest = super::storage::load_conversation(app, id)?;
        if !super::storage::rewrite_conversation_artifact_paths(&mut latest, &mappings) {
            return Ok(latest);
        }
        increment_revision(&mut latest)?;
        latest.updated_at = chrono::Local::now().timestamp();
        self.persist_locked(app, latest).await
    }

    pub async fn prepare_ordinary_workspace(
        &self,
        app: &AppHandle,
        id: &str,
        ordinary_working_root: &str,
    ) -> RepositoryResult<PathBuf> {
        let _barrier = self.barrier.read().await;
        let lock = self.conversation_lock(id);
        let _conversation = lock.lock().await;
        let mut latest = super::storage::load_conversation(app, id)?;
        if super::storage::resolve_conversation_project(app, &latest)?.is_some() {
            return super::storage::resolve_conversation_working_directory(
                app,
                &latest,
                ordinary_working_root,
            )
            .map_err(Into::into);
        }

        let target = crate::native_tools::conversation_workspace_directory(
            ordinary_working_root,
            &latest.id,
        )?;
        let legacy = crate::native_tools::legacy_outputs_dir(&latest.id)?;
        if legacy.exists() {
            crate::native_tools::merge_directory_without_overwrite(&legacy, &target)?;
            if super::storage::rewrite_conversation_artifact_paths(
                &mut latest,
                &[(legacy, target.clone())],
            ) {
                increment_revision(&mut latest)?;
                latest.updated_at = chrono::Local::now().timestamp();
                self.persist_locked(app, latest).await?;
            }
        }
        Ok(target)
    }

    pub async fn create(
        &self,
        app: &AppHandle,
        mut conversation: Conversation,
    ) -> RepositoryResult<Conversation> {
        let _barrier = self.barrier.read().await;
        let lock = self.conversation_lock(&conversation.id);
        let _conversation = lock.lock().await;
        let path = super::storage::conversation_file_path(app, &conversation.id)?;
        if path.exists() {
            return Err(ConversationRepositoryError::Storage(format!(
                "conversation already exists: {}",
                conversation.id
            )));
        }
        conversation.revision = 1;
        self.persist_locked(app, conversation).await
    }

    pub async fn mutate<F>(
        &self,
        app: &AppHandle,
        id: &str,
        mutation: F,
    ) -> RepositoryResult<Conversation>
    where
        F: FnOnce(&mut Conversation) -> Result<(), String>,
    {
        self.mutate_expected(app, id, None, mutation).await
    }

    pub async fn mutate_expected<F>(
        &self,
        app: &AppHandle,
        id: &str,
        expected_revision: Option<u64>,
        mutation: F,
    ) -> RepositoryResult<Conversation>
    where
        F: FnOnce(&mut Conversation) -> Result<(), String>,
    {
        let _barrier = self.barrier.read().await;
        let lock = self.conversation_lock(id);
        let _conversation = lock.lock().await;
        let mut latest = super::storage::load_conversation(app, id)?;
        validate_expected_revision(id, latest.revision, expected_revision)?;
        mutation(&mut latest)?;
        increment_revision(&mut latest)?;
        latest.updated_at = chrono::Local::now().timestamp();
        self.persist_locked(app, latest).await
    }

    async fn persist_locked(
        &self,
        app: &AppHandle,
        conversation: Conversation,
    ) -> RepositoryResult<Conversation> {
        let persisted = super::storage::write_conversation_file(app, &conversation)?;
        let _index = self.index_lock.lock().await;
        // 读侧的 `load_index_or_scan` 只读不写（否则会绕开这把锁 lost update），所以残缺索引
        // 的落盘自愈只能在这里发生：这一步既拿到了对账过的索引，末尾的 save_index 又把它写回。
        let mut index = super::storage::load_index_or_scan(app)?;
        let item = ConversationListItem::from(&persisted);
        if let Some(position) = index
            .conversations
            .iter()
            .position(|candidate| candidate.id == persisted.id)
        {
            index.conversations[position] = item;
        } else {
            index.conversations.insert(0, item);
        }
        super::storage::save_index(app, &index)?;
        Ok(persisted)
    }

    pub async fn append_message(
        &self,
        app: &AppHandle,
        id: &str,
        message: ChatMessage,
    ) -> RepositoryResult<Conversation> {
        self.mutate(app, id, move |conversation| {
            if conversation
                .messages
                .iter()
                .any(|item| item.id == message.id)
            {
                return Err(format!("message already exists: {}", message.id));
            }
            conversation.messages.push(message);
            Ok(())
        })
        .await
    }

    pub async fn upsert_message(
        &self,
        app: &AppHandle,
        id: &str,
        message: ChatMessage,
    ) -> RepositoryResult<Conversation> {
        self.upsert_messages(app, id, vec![message]).await
    }

    pub async fn upsert_messages(
        &self,
        app: &AppHandle,
        id: &str,
        messages: Vec<ChatMessage>,
    ) -> RepositoryResult<Conversation> {
        self.mutate_messages(app, id, None, MessageMutation::Upsert(messages))
            .await
    }

    pub async fn mutate_messages(
        &self,
        app: &AppHandle,
        id: &str,
        expected_revision: Option<u64>,
        mutation: MessageMutation,
    ) -> RepositoryResult<Conversation> {
        self.mutate_expected(app, id, expected_revision, move |conversation| {
            apply_message_mutation(conversation, mutation)
        })
        .await
    }

    pub async fn update_todo(
        &self,
        app: &AppHandle,
        id: &str,
        state: AgentTodoState,
    ) -> RepositoryResult<Conversation> {
        self.mutate(app, id, move |conversation| {
            conversation.agent_todo_state = state;
            Ok(())
        })
        .await
    }

    pub async fn update_plan(
        &self,
        app: &AppHandle,
        id: &str,
        state: AgentPlanState,
    ) -> RepositoryResult<Conversation> {
        self.mutate(app, id, move |conversation| {
            conversation.agent_plan_state = state;
            Ok(())
        })
        .await
    }

    pub async fn update_context(
        &self,
        app: &AppHandle,
        id: &str,
        expected_revision: u64,
        state: ConversationContextState,
    ) -> RepositoryResult<Conversation> {
        self.mutate_expected(app, id, Some(expected_revision), move |conversation| {
            conversation.context_state = state;
            Ok(())
        })
        .await
    }

    pub async fn update_metadata(
        &self,
        app: &AppHandle,
        id: &str,
        mutation: ConversationMetadataMutation,
    ) -> RepositoryResult<Conversation> {
        self.mutate(app, id, move |conversation| {
            match mutation {
                ConversationMetadataMutation::Title(value) => conversation.title = value,
                ConversationMetadataMutation::Model { provider_id, model } => {
                    conversation.provider_id = provider_id;
                    conversation.model = model;
                }
                ConversationMetadataMutation::AgentRuntime(value) => {
                    conversation.agent_runtime = value
                }
                ConversationMetadataMutation::ActiveSkill(value) => {
                    conversation.active_skill_id = value
                }
                ConversationMetadataMutation::Assistant {
                    assistant_id,
                    assistant_snapshot,
                } => {
                    conversation.assistant_id = assistant_id;
                    conversation.assistant_snapshot = assistant_snapshot;
                }
                ConversationMetadataMutation::Pinned(value) => conversation.pinned = value,
                ConversationMetadataMutation::Location {
                    folder,
                    project_id,
                    set_id,
                } => {
                    conversation.folder = folder;
                    conversation.project_id = project_id;
                    conversation.set_id = set_id;
                }
                ConversationMetadataMutation::KnowledgeBases { ids, force_search } => {
                    conversation.knowledge_base_ids = ids;
                    conversation.force_knowledge_search = force_search;
                }
                ConversationMetadataMutation::ThinkingLevel(value) => {
                    conversation.thinking_level = value
                }
                ConversationMetadataMutation::WebSearchMode(value) => {
                    conversation.web_search_mode = value
                }
                ConversationMetadataMutation::ReplyModels(value) => {
                    conversation.reply_models = value
                }
                ConversationMetadataMutation::GroupSelection {
                    group_id,
                    message_id,
                } => {
                    conversation.group_selections.insert(group_id, message_id);
                }
            }
            Ok(())
        })
        .await
    }
}

pub fn repository(app: &AppHandle) -> State<'_, ConversationRepository> {
    app.state::<ConversationRepository>()
}

pub fn repository_error(error: ConversationRepositoryError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation() -> Conversation {
        serde_json::from_value(serde_json::json!({
            "id": "conv_test",
            "revision": 10,
            "title": "test",
            "provider_id": "provider",
            "model": "model",
            "created_at": 1,
            "updated_at": 1,
            "messages": [],
            "agent_todo_state": { "items": [], "updated_at": 42 }
        }))
        .unwrap()
    }

    fn message(id: &str, content: &str) -> ChatMessage {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "role": "assistant",
            "content": content,
            "timestamp": 1
        }))
        .unwrap()
    }

    #[test]
    fn revision_validation_returns_typed_conflict() {
        assert!(validate_expected_revision("conv_test", 10, Some(10)).is_ok());
        assert!(validate_expected_revision("conv_test", 10, None).is_ok());
        assert_eq!(
            validate_expected_revision("conv_test", 11, Some(10)),
            Err(ConversationRepositoryError::Conflict {
                id: "conv_test".to_string(),
                expected: 10,
                actual: 11,
            })
        );
    }

    #[test]
    fn revision_increments_once_and_rejects_overflow() {
        let mut current = conversation();
        increment_revision(&mut current).unwrap();
        assert_eq!(current.revision, 11);
        current.revision = u64::MAX;
        assert!(matches!(
            increment_revision(&mut current),
            Err(ConversationRepositoryError::Storage(_))
        ));
    }

    #[test]
    fn partial_and_final_upsert_share_one_message_without_touching_todo() {
        let mut current = conversation();
        let todo_before = current.agent_todo_state.clone();
        apply_message_mutation(
            &mut current,
            MessageMutation::Upsert(vec![message("msg_run", "partial")]),
        )
        .unwrap();
        apply_message_mutation(
            &mut current,
            MessageMutation::Upsert(vec![message("msg_run", "final")]),
        )
        .unwrap();
        assert_eq!(current.messages.len(), 1);
        assert_eq!(current.messages[0].content, "final");
        assert_eq!(current.agent_todo_state, todo_before);
    }

    #[test]
    fn multi_model_upsert_preserves_all_answers() {
        let mut current = conversation();
        apply_message_mutation(
            &mut current,
            MessageMutation::Upsert(vec![
                message("msg_a", "answer a"),
                message("msg_b", "answer b"),
                message("msg_c", "answer c"),
            ]),
        )
        .unwrap();
        assert_eq!(
            current
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["msg_a", "msg_b", "msg_c"]
        );
    }

    #[test]
    fn keyed_locks_share_only_the_same_conversation() {
        let repository = ConversationRepository::default();
        let first = repository.conversation_lock("conv_a");
        let same = repository.conversation_lock("conv_a");
        let other = repository.conversation_lock("conv_b");
        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
    }
}
