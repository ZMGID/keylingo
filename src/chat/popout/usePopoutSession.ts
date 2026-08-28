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
  reset as resetStreamStore,
  setCoarse as setStreamCoarse,
  setSnapshot as setStreamSnapshot,
  useStreamCoarse,
} from '../streamingStore'
import type { InputBarProps } from '../InputBar'
import type { MessageListProps } from '../MessageList'
import type { Conversation, PendingAttachment, ThinkingLevel } from '../types'
import type {
  ChatSessionConsentPayload,
  ChatToolConfirmPayload,
  ChatUserPromptPayload,
} from '../../api/tauri'
import type { Lang } from '../../settings/i18n'

export function usePopoutSession(conversationId: string, lang: Lang) {
  const [conversation, setConversation] = useState<Conversation | null>(null)
  const [loadError, setLoadError] = useState('')
  const [pendingUserMessage, setPendingUserMessage] = useState<Conversation['messages'][number] | null>(null)
  const [pendingToolConfirm, setPendingToolConfirm] = useState<ChatToolConfirmPayload | null>(null)
  const [pendingSessionConsent, setPendingSessionConsent] = useState<ChatSessionConsentPayload | null>(null)
  const [pendingUserPrompt, setPendingUserPrompt] = useState<ChatUserPromptPayload | null>(null)
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
      const restored = createEmptyStreamSnapshot()
      restored.runId = payload.runId
      restored.messageId = payload.messageId
      restored.streaming = true
      restored.startedAt = Date.now()
      snapshotRef.current = restored
      if (!inFlightRef.current) restoredRunIdsRef.current.add(payload.runId)
      inFlightRef.current = true
      setPendingToolConfirm(null)
      setPendingSessionConsent(null)
      showStreamSnapshotIfCurrent(payload.conversationId, restored)
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
    const snapshot = snapshotRef.current ?? createEmptyStreamSnapshot()
    snapshotRef.current = snapshot
    applyToolRecordToSnapshot(snapshot, toolEventToRecord(payload))
    showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatSubagent, (payload) => {
    const snapshot = snapshotRef.current
    if (!snapshot || payload.parentConversationId !== conversationIdRef.current) return
    const index = findSubagentToolIndex(snapshot.toolCalls, payload)
    if (index < 0) return
    snapshot.toolCalls = snapshot.toolCalls.map((tool, i) => (
      i === index ? mergeSubagentProgress(tool, payload) : tool
    ))
    showStreamSnapshotIfCurrent(payload.parentConversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatToolConfirm, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    setPendingToolConfirm(payload)
    setToolConfirmError('')
  }, [])

  useTauriEvent(api.onChatToolConfirmWithdraw, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    setPendingToolConfirm((current) => current?.toolCallId === payload.toolCallId ? null : current)
  }, [])

  useTauriEvent(api.onChatSessionConsent, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    setPendingSessionConsent(payload)
    setSessionConsentError('')
  }, [])

  useTauriEvent(api.onChatUserPrompt, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    setPendingUserPrompt(payload)
  }, [])

  useTauriEvent(api.onChatStatusNote, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    const snapshot = snapshotRef.current
    if (!snapshot) return
    snapshot.statusNote = payload.note
    showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

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
      setPendingToolConfirm(null)
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

  const inputBarProps: InputBarProps = {
    onSend: handleSend,
    disabled: streamCoarse.streaming,
    onCancel: handleCancel,
    cancelVisible: streamCoarse.streaming,
    cancelling: streamCoarse.cancelling,
    autoFocus: true,
    conversationId,
    usesChatRuntime,
    usesExternalRuntime,
    externalAgentName: runtime.externalAgentId ?? null,
    modeOptions: [],
    showProjectEntry: false,
  }

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
    dismissUserPrompt: () => setPendingUserPrompt(null),
    handleModelChange,
    handleThinkingLevelChange,
    handleRuntimeChange,
    handleExternalModelChange,
    handleApprovalPolicyChange,
  }
}
