import { useCallback, useRef } from 'react'
import { api, type ChatExternalSendRequest } from '../../api/tauri'
import type { PendingAttachment } from '../types'

interface UseExternalSendQueueParams {
  /** 取到消息后先切回会话视图。 */
  onEnterConversationView: () => void
  /** 历史预置分支：把整段多轮历史搬成新会话，不发消息。 */
  onImportConversation: (
    messages: NonNullable<ChatExternalSendRequest['messages']>,
    attachmentPaths: string[],
  ) => Promise<unknown>
  /** 发送一条外部消息；返回 false 表示当前发不出去（如正在生成），需重排。 */
  onSendMessage: (
    content: string,
    attachments: PendingAttachment[],
    options: { forceNewConversation: true },
  ) => Promise<boolean>
  onError: (message: string) => void
}

/**
 * 外部发送队列（如 Lens 交接过来的消息）。
 *
 * 三个 ref 只服务这一件事，故整体搬出：
 *   - queue：已取走但尚未发出的请求
 *   - processing：单飞标志，避免并发 drain
 *   - requested：drain 期间又被触发时的重排标志
 *
 * 返回的 drainExternalSends 身份稳定（依赖数组为空），调用方的 effect 不会因它重订阅 ——
 * 这是搬迁前就有的性质，靠参数回调经 ref 间接调用来保持。
 */
export function useExternalSendQueue({
  onEnterConversationView,
  onImportConversation,
  onSendMessage,
  onError,
}: UseExternalSendQueueParams) {
  const queueRef = useRef<ChatExternalSendRequest[]>([])
  const processingRef = useRef(false)
  const requestedRef = useRef(false)

  // 参数回调每次渲染都是新身份；经 ref 读取以保持 drain 本身稳定。
  const callbacksRef = useRef({ onEnterConversationView, onImportConversation, onSendMessage, onError })
  callbacksRef.current = { onEnterConversationView, onImportConversation, onSendMessage, onError }

  const drainExternalSends = useCallback(async () => {
    if (processingRef.current) {
      requestedRef.current = true
      return
    }

    processingRef.current = true
    try {
      do {
        requestedRef.current = false

        const result = await api.chatTakeExternalSends()
        if (!result.success) {
          const error = 'error' in result && typeof result.error === 'string'
            ? result.error
            : ''
          throw new Error(error || 'Failed to take external Chat messages')
        }
        const requests = result.requests ?? []
        if (requests.length > 0) {
          queueRef.current.push(...requests)
        }

        const request = queueRef.current[0]
        if (!request) continue
        callbacksRef.current.onEnterConversationView()
        const attachmentPaths = (request.attachments ?? [])
          .map((attachment) => attachment.path)
          .filter((path): path is string => !!path)

        // 历史预置分支：把 Lens 完整多轮历史 + 截图搬成一个新会话（不发消息、不触发回复），落地末尾可续聊。
        if (request.messages && request.messages.length > 0) {
          await callbacksRef.current.onImportConversation(request.messages, attachmentPaths)
          queueRef.current.shift()
          continue
        }

        const attachments = (request.attachments ?? [])
          .filter((attachment) => attachment.path)
          .map<PendingAttachment>((attachment, index) => ({
            id: attachment.id || `external-${request.id}-${index}`,
            type: attachment.type === 'file' ? 'file' : 'image',
            name: attachment.name || (attachment.type === 'file' ? 'Attachment' : 'Image'),
            path: attachment.path,
          }))
        const accepted = await callbacksRef.current.onSendMessage(
          request.content ?? '',
          attachments,
          { forceNewConversation: true },
        )
        if (accepted) {
          queueRef.current.shift()
        } else {
          requestedRef.current = true
          break
        }
      } while (requestedRef.current || queueRef.current.length > 0)
    } catch (err) {
      console.error('Failed to process external Chat message:', err)
      callbacksRef.current.onError(
        typeof err === 'string' ? err : (err as Error).message || '外部消息发送失败',
      )
    } finally {
      processingRef.current = false
      if (requestedRef.current) {
        window.setTimeout(() => {
          void drainExternalSends()
        }, 0)
      }
    }
  }, [])

  /** 流式结束后调用方据此判断要不要补一次 drain（搬迁前是直接读 ref）。 */
  const hasPendingDrainRequest = useCallback(() => requestedRef.current, [])

  return { drainExternalSends, hasPendingDrainRequest }
}
