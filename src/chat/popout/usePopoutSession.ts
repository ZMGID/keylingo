import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { api } from '../../api/tauri'
import { syncChatProtocol } from '../../api/chatProtocol'
import { getSettingsCached, refreshSettings, saveSettingsCached } from '../../api/settingsCache'
import {
  agentRuntimesEqual,
  chatApi,
  normalizeAgentRuntime,
  type AgentRuntimeConfig,
} from '../api'
import { useStreamRenderFrame } from '../hooks/useStreamRenderFrame'
import { useTauriEvent } from '../hooks/useTauriEvent'
import {
  applyStreamDeltaToSnapshot,
  applyToolRecordToSnapshot,
  findSubagentToolIndex,
  finalizeReasoningDurationOnDone,
  isStreamTerminal,
  mergeSubagentProgress,
  streamPayloadToSegment,
  streamReasoningDelta,
  streamTextDelta,
  toolEventToRecord,
  userPromptEventToRecord,
} from '../streamApply'
import { createEmptyStreamSnapshot, type ConversationStreamSnapshot } from '../conversationRuns'
import {
  beginGroup,
  endGroup,
  ensureGroupColumn,
  flushGroups,
  getActiveGroup,
  hasActiveGroup,
  restoreGroupArm,
  touchGroup,
} from '../groupStreamingStore'
import {
  reset as resetStreamStore,
  setCoarse as setStreamCoarse,
  setSnapshot as setStreamSnapshot,
  useStreamCoarse,
} from '../streamingStore'
import type { MessageListProps } from '../MessageList'
import type { AgentPlanState, AgentTodoState, Conversation, PendingAttachment, ThinkingLevel } from '../types'
import { insertTextIntoComposer } from '../composerInsert'
import { usePopoutComposer } from './usePopoutComposer'
import type {
  ChatSessionConsentPayload,
  ChatToolConfirmPayload,
  ChatUserPromptPayload,
  ChatHookPayload,
} from '../../api/tauri'
import type { Lang } from '../../settings/i18n'

export function usePopoutSession(conversationId: string, lang: Lang) {
  const [conversation, setConversation] = useState<Conversation | null>(null)
  const [loadError, setLoadError] = useState('')
  const [pendingUserMessage, setPendingUserMessage] = useState<Conversation['messages'][number] | null>(null)
  const [pendingToolConfirm, setPendingToolConfirm] = useState<ChatToolConfirmPayload | null>(null)
  const [pendingSessionConsent, setPendingSessionConsent] = useState<ChatSessionConsentPayload | null>(null)
  const [pendingUserPrompt, setPendingUserPrompt] = useState<ChatUserPromptPayload | null>(null)
  const [hookWarning, setHookWarning] = useState<ChatHookPayload | null>(null)
  const [toolConfirmError, setToolConfirmError] = useState('')
  const [sessionConsentError, setSessionConsentError] = useState('')
  const [toolConfirmSubmitting, setToolConfirmSubmitting] = useState(false)
  const [sessionConsentSubmitting, setSessionConsentSubmitting] = useState(false)
  const [approvalPolicy, setApprovalPolicy] = useState('readonly_auto_sensitive_confirm')

  const conversationIdRef = useRef(conversationId)
  conversationIdRef.current = conversationId
  const snapshotRef = useRef<ConversationStreamSnapshot | null>(null)
  const inFlightRef = useRef(false)
  const pendingDoneRef = useRef<(() => Promise<void>) | null>(null)
  const restoredRunIdsRef = useRef(new Set<string>())
  const pendingToolConfirmsRef = useRef<ChatToolConfirmPayload[]>([])
  const pendingUserPromptsRef = useRef<ChatUserPromptPayload[]>([])
  const streamCoarse = useStreamCoarse()

  const applySnapshot = useCallback((snapshot: ConversationStreamSnapshot) => {
    setStreamSnapshot(snapshot)
    setStreamCoarse({ streaming: snapshot.streaming, cancelling: false })
  }, [])

  const { showStreamSnapshotIfCurrent, cancelPendingFrame, flushStreamRender } = useStreamRenderFrame({
    applySnapshot,
    currentConversationIdRef: conversationIdRef,
  })

  const reload = useCallback(async () => {
    const conv = await chatApi.getConversation(conversationId)
    if (conversationIdRef.current !== conversationId) return
    setConversation(conv)
    setLoadError('')
  }, [conversationId])

  useEffect(() => {
    let cancelled = false
    setLoadError('')
    void chatApi.getConversation(conversationId).then((conv) => {
      if (cancelled) return
      setConversation(conv)
      void syncChatProtocol(conversationId).catch(() => {})
    }).catch((err) => {
      if (cancelled) return
      setLoadError(typeof err === 'string' ? err : (err as Error).message || '加载对话失败')
    })
    void getSettingsCached().then((settings) => {
      if (!cancelled && settings.chatTools?.approvalPolicy) {
        setApprovalPolicy(settings.chatTools.approvalPolicy)
      }
    }).catch(() => {})
    return () => {
      cancelled = true
      cancelPendingFrame()
      endGroup(conversationId)
      resetStreamStore()
    }
  }, [cancelPendingFrame, conversationId])

  const settlePreview = useCallback(() => {
    flushStreamRender()
    snapshotRef.current = null
    resetStreamStore()
    setStreamCoarse({ streaming: false, streamFrozen: false, cancelling: false, streamError: '' })
  }, [flushStreamRender])

  const finishRun = useCallback(async () => {
    try {
      await reload()
    } catch (err) {
      setStreamCoarse({
        streamError: typeof err === 'string' ? err : (err as Error).message || '同步对话失败',
      })
    }
    settlePreview()
  }, [reload, settlePreview])

  useTauriEvent(api.onChatStream, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const terminal = isStreamTerminal(payload)
    if (payload.type === 'run_started') {
      setHookWarning(null)
      if (payload.recovery) {
        restoreGroupArm(
          payload.conversationId,
          payload.recovery.groupId,
          payload.recovery.groupSize,
          payload.recovery.armIndex,
          payload.messageId,
          payload.recovery.providerId,
          payload.recovery.model,
        )
      }
      if (!inFlightRef.current) restoredRunIdsRef.current.add(payload.runId)
      inFlightRef.current = true
      setPendingSessionConsent(null)
      if (hasActiveGroup(payload.conversationId) && payload.messageId) {
        const column = ensureGroupColumn(payload.conversationId, payload.messageId)
        if (column) {
          Object.assign(column, createEmptyStreamSnapshot(), {
            runId: payload.runId,
            messageId: payload.messageId,
            streaming: true,
            startedAt: Date.now(),
          })
          touchGroup(payload.conversationId)
        }
        setStreamCoarse({ streaming: true, streamError: '', cancelling: false })
        return
      }
      const restored = createEmptyStreamSnapshot()
      restored.runId = payload.runId
      restored.messageId = payload.messageId
      restored.streaming = true
      restored.startedAt = Date.now()
      snapshotRef.current = restored
      pendingToolConfirmsRef.current = []
      pendingUserPromptsRef.current = []
      setPendingToolConfirm(null)
      showStreamSnapshotIfCurrent(payload.conversationId, restored)
      return
    }
    if (hasActiveGroup(payload.conversationId) && payload.messageId) {
      const column = ensureGroupColumn(payload.conversationId, payload.messageId)
      if (!column) return
      const segment = streamPayloadToSegment(payload)
      if (streamTextDelta(payload) || streamReasoningDelta(payload)) column.statusNote = null
      applyStreamDeltaToSnapshot(column, payload, segment)
      if (terminal) {
        finalizeReasoningDurationOnDone(column)
        column.streaming = false
        flushGroups(payload.conversationId)
        restoredRunIdsRef.current.delete(payload.runId)
        const group = getActiveGroup(payload.conversationId)
        if (group?.columns.every((item) => !item.streaming)) {
          endGroup(payload.conversationId)
          if (restoredRunIdsRef.current.size > 0 || !inFlightRef.current) void finishRun()
          else pendingDoneRef.current = finishRun
        }
      } else {
        touchGroup(payload.conversationId)
      }
      return
    }
    if (!snapshotRef.current && !inFlightRef.current) {
      if (terminal) void finishRun()
      return
    }
    const snapshot = snapshotRef.current ?? createEmptyStreamSnapshot()
    snapshotRef.current = snapshot
    if (payload.runId) {
      if (snapshot.runId && snapshot.runId !== payload.runId) return
      snapshot.runId = payload.runId
    }
    if (payload.messageId) snapshot.messageId = payload.messageId
    const segment = streamPayloadToSegment(payload)
    if (streamTextDelta(payload) || streamReasoningDelta(payload)) snapshot.statusNote = null
    applyStreamDeltaToSnapshot(snapshot, payload, segment)
    showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
    if (terminal) {
      finalizeReasoningDurationOnDone(snapshot)
      snapshot.streaming = false
      showStreamSnapshotIfCurrent(payload.conversationId, snapshot, true)
      if (restoredRunIdsRef.current.delete(payload.runId) || !inFlightRef.current) {
        void finishRun()
        return
      }
      pendingDoneRef.current = finishRun
    }
  }, [finishRun, showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatTool, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    if (hasActiveGroup(payload.conversationId) && payload.messageId) {
      const column = ensureGroupColumn(payload.conversationId, payload.messageId)
      if (!column) return
      applyToolRecordToSnapshot(column, toolEventToRecord(payload))
      touchGroup(payload.conversationId)
      return
    }
    const snapshot = snapshotRef.current ?? createEmptyStreamSnapshot()
    snapshotRef.current = snapshot
    applyToolRecordToSnapshot(snapshot, toolEventToRecord(payload))
    showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatSubagent, (payload) => {
    if (payload.parentConversationId !== conversationIdRef.current) return
    if (hasActiveGroup(payload.parentConversationId)) {
      const group = getActiveGroup(payload.parentConversationId)
      const column = group?.columns.find((item) => (
        payload.parentRunId ? item.runId === payload.parentRunId : true
      ))
      if (!column) return
      const index = findSubagentToolIndex(column.toolCalls, payload)
      if (index < 0) return
      column.toolCalls = column.toolCalls.map((tool, i) => (
        i === index ? mergeSubagentProgress(tool, payload) : tool
      ))
      touchGroup(payload.parentConversationId)
      return
    }
    const snapshot = snapshotRef.current
    if (!snapshot) return
    const index = findSubagentToolIndex(snapshot.toolCalls, payload)
    if (index < 0) return
    snapshot.toolCalls = snapshot.toolCalls.map((tool, i) => (
      i === index ? mergeSubagentProgress(tool, payload) : tool
    ))
    showStreamSnapshotIfCurrent(payload.parentConversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatToolConfirm, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const queue = pendingToolConfirmsRef.current
    if (!queue.some((item) => item.toolCallId === payload.toolCallId)) queue.push(payload)
    setPendingToolConfirm(queue[0] ?? null)
    setToolConfirmError('')
  }, [])

  useTauriEvent(api.onChatToolConfirmWithdraw, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const rest = pendingToolConfirmsRef.current.filter((item) => item.toolCallId !== payload.toolCallId)
    pendingToolConfirmsRef.current = rest
    setPendingToolConfirm((current) => (
      current?.toolCallId === payload.toolCallId ? rest[0] ?? null : current
    ))
  }, [])

  useTauriEvent(api.onChatSessionConsent, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    setPendingSessionConsent(payload)
    setSessionConsentError('')
  }, [])

  useTauriEvent(api.onChatUserPrompt, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const queue = pendingUserPromptsRef.current
    if (!queue.some((item) => item.toolCallId === payload.toolCallId)) queue.push(payload)
    setPendingUserPrompt(queue[0] ?? null)
  }, [])

  useTauriEvent(api.onChatHook, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    setHookWarning(payload)
  }, [])

  useTauriEvent(api.onChatQueuedTextsRestored, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const text = payload.texts.map((item) => item.trim()).filter(Boolean).join('\n\n')
    if (text) insertTextIntoComposer(text)
  }, [])

  useTauriEvent(api.onChatStatusNote, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    if (hasActiveGroup(payload.conversationId)) {
      const group = getActiveGroup(payload.conversationId)
      if (!group) return
      for (const column of group.columns) {
        if (column.streaming) column.statusNote = payload.note
      }
      touchGroup(payload.conversationId)
      return
    }
    const snapshot = snapshotRef.current
    if (!snapshot) return
    snapshot.statusNote = payload.note
    showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatTodo, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const todoState = payload.todoState as AgentTodoState
    setConversation((current) => current
      ? { ...current, agent_todo_state: todoState, agentTodoState: todoState }
      : current)
  }, [])

  useTauriEvent(api.onChatPlan, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const planState = payload.planState as AgentPlanState
    setConversation((current) => current
      ? { ...current, agent_plan_state: planState, agentPlanState: planState }
      : current)
  }, [])

  const handleSend = useCallback(async (
    content: string,
    attachments: PendingAttachment[] = [],
    options?: { onAccepted?: () => void },
  ) => {
    const trimmed = content.trim()
    if (!trimmed && attachments.length === 0) return false
    const conv = conversation
    if (!conv) return false
    inFlightRef.current = true
    setHookWarning(null)
    const replyArms = conv.reply_models ?? conv.replyModels ?? []
    const convPlanMode =
      conv.agent_plan_state?.mode ?? conv.agentPlanState?.mode ?? 'act'
    if (replyArms.length >= 2 && convPlanMode === 'act') {
      beginGroup(
        conversationId,
        `grp-local-${Date.now()}`,
        replyArms.map((ref) => ({ providerId: ref.provider_id, model: ref.model })),
      )
    }
    setPendingUserMessage({
      id: `pending_${Date.now()}`,
      role: 'user',
      content: trimmed,
      attachments: attachments.map((attachment) => ({
        id: attachment.id,
        type: attachment.type,
        name: attachment.name,
        path: attachment.path,
      })),
      timestamp: Math.floor(Date.now() / 1000),
    })
    setStreamCoarse({ streaming: true, streamError: '', cancelling: false })
    options?.onAccepted?.()
    let persisted: Conversation | null = null
    try {
      persisted = await chatApi.sendMessage(
        conversationId,
        trimmed,
        attachments,
        conv.active_skill_id ?? conv.activeSkillId,
      )
      setConversation(persisted)
      setPendingUserMessage(null)
    } catch (err) {
      const kept = (err as { conversation?: Conversation })?.conversation
      setPendingUserMessage(null)
      if (kept) setConversation(kept)
      setStreamCoarse({
        streamError: typeof err === 'string' ? err : (err as Error).message || '发送失败',
      })
    } finally {
      inFlightRef.current = false
      endGroup(conversationId)
      const delayed = pendingDoneRef.current
      pendingDoneRef.current = null
      if (persisted || !delayed) settlePreview()
      else await delayed()
    }
    return true
  }, [conversation, conversationId, settlePreview])

  const handleCancel = useCallback(async () => {
    setStreamCoarse({ cancelling: true })
    try {
      await chatApi.cancelStream(conversationId)
    } catch (err) {
      console.error('Failed to cancel stream:', err)
      setStreamCoarse({ cancelling: false })
    }
  }, [conversationId])

  const resolveToolConfirm = useCallback(async (
    approved: boolean,
    always = false,
    permissionMode: string | null = null,
  ) => {
    const prompt = pendingToolConfirm
    if (!prompt) return
    setToolConfirmSubmitting(true)
    setToolConfirmError('')
    try {
      await api.chatConfirmToolCall(prompt.toolCallId, approved, always, permissionMode)
      const rest = pendingToolConfirmsRef.current.filter((item) => item.toolCallId !== prompt.toolCallId)
      pendingToolConfirmsRef.current = rest
      setPendingToolConfirm(rest[0] ?? null)
    } catch (error) {
      setToolConfirmError(typeof error === 'string' ? error : (error as Error).message || '提交审批失败')
    } finally {
      setToolConfirmSubmitting(false)
    }
  }, [pendingToolConfirm])

  const resolveSessionConsent = useCallback(async (granted: boolean) => {
    const prompt = pendingSessionConsent
    if (!prompt) return
    setSessionConsentSubmitting(true)
    try {
      await api.chatRespondSessionConsent(prompt.conversationId, granted)
      setPendingSessionConsent(null)
    } catch (error) {
      setSessionConsentError(typeof error === 'string' ? error : (error as Error).message || '提交会话授权失败')
    } finally {
      setSessionConsentSubmitting(false)
    }
  }, [pendingSessionConsent])

  const handleModelChange = useCallback(async (providerId: string, model: string) => {
    const next = await chatApi.updateConversation(conversationId, { providerId, model })
    setConversation(next)
  }, [conversationId])

  const handleThinkingLevelChange = useCallback(async (level: ThinkingLevel | null) => {
    const next = await chatApi.updateConversation(conversationId, { thinkingLevel: level })
    setConversation(next)
  }, [conversationId])

  const handleRuntimeChange = useCallback(async (runtime: AgentRuntimeConfig) => {
    if (conversation && agentRuntimesEqual(normalizeAgentRuntime(conversation), runtime)) return
    const next = await chatApi.setAgentRuntime(conversationId, runtime)
    setConversation(next)
  }, [conversation, conversationId])

  const handleExternalModelChange = useCallback(async (model: string, reasoning?: string | null) => {
    const current = normalizeAgentRuntime(conversation)
    await handleRuntimeChange({
      ...current,
      kind: 'external',
      externalModel: model,
      externalReasoning: reasoning ?? current.externalReasoning ?? null,
    })
  }, [conversation, handleRuntimeChange])

  const handleApprovalPolicyChange = useCallback(async (nextApprovalPolicy: string) => {
    setApprovalPolicy(nextApprovalPolicy)
    try {
      const settings = await refreshSettings()
      await saveSettingsCached({
        ...settings,
        chatTools: {
          ...settings.chatTools,
          approvalPolicy: nextApprovalPolicy,
        },
      })
    } catch (err) {
      console.error('Failed to update approval policy:', err)
    }
  }, [])

  const pendingUserPromptRecord = pendingUserPrompt ? userPromptEventToRecord(pendingUserPrompt) : null
  const runtime = normalizeAgentRuntime(conversation)
  const usesChatRuntime = runtime.kind === 'chat'
  const usesExternalRuntime = runtime.kind === 'external'

  const displayMessages = useMemo(() => {
    const messages = conversation?.messages ?? []
    if (!pendingUserMessage) return messages
    if (messages.some((item) => item.id === pendingUserMessage.id)) return messages
    return [...messages, pendingUserMessage]
  }, [conversation?.messages, pendingUserMessage])

  const inputBarProps = usePopoutComposer({
    conversation,
    setConversation,
    conversationId,
    lang,
    displayMessages,
    streaming: streamCoarse.streaming,
    usesChatRuntime,
    usesExternalRuntime,
    runtime,
    onSend: handleSend,
    onCancel: handleCancel,
    cancelVisible: streamCoarse.streaming,
    cancelling: streamCoarse.cancelling,
    disabled: streamCoarse.streaming,
  })

  const messageListProps: MessageListProps = {
    conversationId,
    messages: displayMessages,
    lang,
    sessionProviderId: conversation?.provider_id,
    sessionModel: conversation?.model,
  }

  return {
    conversation,
    loadError,
    runtime,
    usesChatRuntime,
    usesExternalRuntime,
    approvalPolicy,
    inputBarProps,
    messageListProps,
    streamError: streamCoarse.streamError,
    pendingToolConfirm,
    pendingSessionConsent,
    pendingUserPrompt,
    pendingUserPromptRecord,
    toolConfirmError,
    sessionConsentError,
    toolConfirmSubmitting,
    sessionConsentSubmitting,
    resolveToolConfirm,
    resolveSessionConsent,
    dismissUserPrompt: () => {
      const current = pendingUserPrompt
      const rest = pendingUserPromptsRef.current.filter((item) => (
        current ? item.toolCallId !== current.toolCallId : true
      ))
      pendingUserPromptsRef.current = rest
      setPendingUserPrompt(rest[0] ?? null)
    },
    hookWarning,
    dismissHookWarning: () => setHookWarning(null),
    handleModelChange,
    handleThinkingLevelChange,
    handleRuntimeChange,
    handleExternalModelChange,
    handleApprovalPolicyChange,
  }
}
