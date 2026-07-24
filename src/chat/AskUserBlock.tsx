import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  CheckCircle2,
  CheckSquare2,
  Circle,
  Loader2,
  MessageSquareMore,
  Send,
  SkipForward,
  Square,
  XCircle,
} from 'lucide-react'
import { api } from '../api/tauri'
import { Button } from '../components/Button'
import type {
  AskUserAnswer,
  AskUserOption,
  AskUserPhase,
  AskUserQuestion,
  AskUserStructuredContent,
  ToolCallRecord,
} from './types'

interface AskUserBlockProps {
  toolCall: ToolCallRecord
}

interface DraftAnswer {
  selectedOptionIds: string[]
  customText: string
}

interface ParsedAskUser {
  title: string
  phase: AskUserPhase | string
  questions: AskUserQuestion[]
  answers: Record<string, AskUserAnswer>
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function compactText(text: string, max = 180): string {
  const cleaned = text.replace(/\s+/g, ' ').trim()
  if (cleaned.length <= max) return cleaned
  return `${cleaned.slice(0, max).trimEnd()}...`
}

function parsedArguments(toolCall: ToolCallRecord): Record<string, unknown> | null {
  const value = toolCall.arguments ?? toolCall.args ?? toolCall.input
  if (!value) return null
  if (typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>
  if (typeof value !== 'string') return null
  try {
    const parsed = JSON.parse(value)
    return objectValue(parsed)
  } catch {
    return null
  }
}

function normalizeQuestions(value: unknown): AskUserQuestion[] {
  if (!Array.isArray(value)) return []
  const questions: AskUserQuestion[] = []
  for (const item of value) {
    const question = objectValue(item)
    if (!question) continue
    const id = typeof question.id === 'string' ? question.id.trim() : ''
    const prompt = typeof question.prompt === 'string' ? question.prompt.trim() : ''
    const options = Array.isArray(question.options)
      ? question.options
        .map((option) => normalizeOption(option))
        .filter((option): option is AskUserOption => Boolean(option))
      : []
    if (!id || !prompt || options.length < 2) continue
    const multiple = question.allow_multiple === true || question.allowMultiple === true
    const custom = question.allow_custom === true || question.allowCustom === true
    questions.push({
      id,
      prompt,
      options,
      allow_multiple: multiple,
      allowMultiple: multiple,
      allow_custom: custom,
      allowCustom: custom,
    })
  }
  return questions
}

function normalizeOption(value: unknown): AskUserOption | null {
  const option = objectValue(value)
  if (!option) return null
  const id = typeof option.id === 'string' ? option.id.trim() : ''
  const label = typeof option.label === 'string' ? option.label.trim() : ''
  const description = typeof option.description === 'string' ? option.description.trim() : ''
  if (!id || !label) return null
  return {
    id,
    label,
    description: description || undefined,
  }
}

function normalizeAnswers(value: unknown): Record<string, AskUserAnswer> {
  const raw = objectValue(value)
  if (!raw) return {}
  return Object.fromEntries(
    Object.entries(raw).map(([questionId, answer]) => {
      const normalized = normalizeAnswer(answer)
      return [questionId, normalized]
    }),
  )
}

function normalizeAnswer(value: unknown): AskUserAnswer {
  const answer = objectValue(value)
  if (!answer) return { selected_option_ids: [], selectedOptionIds: [], custom_text: null, customText: null }
  const selected = Array.isArray(answer.selected_option_ids)
    ? answer.selected_option_ids
    : Array.isArray(answer.selectedOptionIds)
      ? answer.selectedOptionIds
      : []
  const selectedOptionIds = selected
    .filter((item): item is string => typeof item === 'string')
    .map((item) => item.trim())
    .filter(Boolean)
  const customText = typeof answer.custom_text === 'string'
    ? answer.custom_text
    : typeof answer.customText === 'string'
      ? answer.customText
      : ''
  return {
    selected_option_ids: selectedOptionIds,
    selectedOptionIds,
    custom_text: customText.trim() || null,
    customText: customText.trim() || null,
  }
}

function parseAskUser(toolCall: ToolCallRecord): ParsedAskUser | null {
  const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent) as AskUserStructuredContent | null
  const askUser = objectValue(structured?.askUser)
  if (askUser) {
    const questions = normalizeQuestions(askUser.questions)
    if (questions.length > 0) {
      return {
        title: typeof askUser.title === 'string' && askUser.title.trim()
          ? askUser.title.trim()
          : '需要确认',
        phase: typeof askUser.phase === 'string' ? askUser.phase : 'awaiting',
        questions,
        answers: normalizeAnswers(askUser.answers),
      }
    }
  }

  const args = parsedArguments(toolCall)
  const questions = normalizeQuestions(args?.questions)
  if (!questions.length) return null
  return {
    title: typeof args?.title === 'string' && args.title.trim() ? args.title.trim() : '需要确认',
    phase: toolCall.status === 'cancelled' ? 'cancelled' : 'awaiting',
    questions,
    answers: {},
  }
}

function answerSelectedIds(answer?: AskUserAnswer): string[] {
  return answer?.selected_option_ids ?? answer?.selectedOptionIds ?? []
}

function answerCustomText(answer?: AskUserAnswer): string {
  return answer?.custom_text ?? answer?.customText ?? ''
}

function allowMultiple(question: AskUserQuestion): boolean {
  return question.allow_multiple === true || question.allowMultiple === true
}

function allowCustom(question: AskUserQuestion): boolean {
  return question.allow_custom === true || question.allowCustom === true
}

function draftHasAnswer(question: AskUserQuestion, answer?: DraftAnswer): boolean {
  if (!answer) return false
  const hasSelection = answer.selectedOptionIds.length > 0
  const hasCustom = allowCustom(question) && answer.customText.trim().length > 0
  return hasSelection || hasCustom
}

function createDraft(parsed: ParsedAskUser | null): Record<string, DraftAnswer> {
  if (!parsed) return {}
  return Object.fromEntries(parsed.questions.map((question) => {
    const answer = parsed.answers[question.id]
    return [
      question.id,
      {
        selectedOptionIds: answerSelectedIds(answer),
        customText: answerCustomText(answer),
      },
    ]
  }))
}

function firstUnansweredIndex(parsed: ParsedAskUser | null, draft: Record<string, DraftAnswer>): number {
  if (!parsed) return 0
  const index = parsed.questions.findIndex((question) => !draftHasAnswer(question, draft[question.id]))
  return index >= 0 ? index : 0
}

function promptSignature(parsed: ParsedAskUser | null): string {
  if (!parsed) return ''
  return parsed.questions
    .map((question) => `${question.id}:${question.options.map((option) => option.id).join(',')}`)
    .join('|')
}

function phaseLabel(phase: string): string {
  switch (phase) {
    case 'answered':
      return '已回答'
    case 'skipped':
      return '已跳过'
    case 'timeout':
      return '已超时'
    case 'cancelled':
      return '已取消'
    default:
      return '等待'
  }
}

function optionLabel(question: AskUserQuestion, optionId: string): string {
  return question.options.find((option) => option.id === optionId)?.label ?? optionId
}

function readonlySummary(question: AskUserQuestion, answer?: AskUserAnswer): string {
  const labels = answerSelectedIds(answer).map((optionId) => optionLabel(question, optionId))
  const custom = answerCustomText(answer)
  return [...labels, custom].filter(Boolean).join(' · ') || '未回答'
}

function phaseTone(phase: string): string {
  switch (phase) {
    case 'answered':
      return 'text-emerald-600 dark:text-emerald-400'
    case 'skipped':
    case 'timeout':
    case 'cancelled':
      return 'text-neutral-400 dark:text-neutral-500'
    default:
      return 'text-neutral-400 dark:text-neutral-500'
  }
}

function preventMouseFocus(event: ReactMouseEvent<HTMLButtonElement>) {
  event.preventDefault()
}

export function AskUserBlock({ toolCall }: AskUserBlockProps) {
  const parsed = useMemo(() => parseAskUser(toolCall), [toolCall])
  const signature = promptSignature(parsed)
  const optionsScrollRef = useRef<HTMLDivElement>(null)
  const [draft, setDraft] = useState<Record<string, DraftAnswer>>(() => createDraft(parsed))
  const [currentIndex, setCurrentIndex] = useState(() => firstUnansweredIndex(parsed, createDraft(parsed)))
  const [submitting, setSubmitting] = useState(false)
  const [submitError, setSubmitError] = useState('')
  const questionCount = parsed?.questions.length ?? 0
  const visibleIndex = questionCount > 0 ? Math.min(currentIndex, questionCount - 1) : 0

  useEffect(() => {
    const nextDraft = createDraft(parsed)
    setDraft(nextDraft)
    setCurrentIndex(firstUnansweredIndex(parsed, nextDraft))
    setSubmitError('')
  }, [toolCall.id, signature, parsed])

  useLayoutEffect(() => {
    const el = optionsScrollRef.current
    if (el) el.scrollTop = 0
  }, [toolCall.id, visibleIndex])

  if (!parsed) {
    return (
      <div className="not-prose mb-2 inline-flex max-w-full items-center gap-1.5 rounded-md py-0.5 text-[11.5px] leading-5 text-neutral-400 dark:text-neutral-500">
        <MessageSquareMore size={12} strokeWidth={1.9} className="shrink-0" />
        <span className="truncate">等待用户确认</span>
      </div>
    )
  }

  const awaiting = parsed.phase === 'awaiting'
  const currentQuestion = parsed.questions[visibleIndex]
  const currentAnswer = currentQuestion
    ? draft[currentQuestion.id] ?? { selectedOptionIds: [], customText: '' }
    : { selectedOptionIds: [], customText: '' }
  const answeredCount = parsed.questions.filter((question) => draftHasAnswer(question, draft[question.id])).length
  const allAnswered = answeredCount === parsed.questions.length
  const currentAnswered = currentQuestion ? draftHasAnswer(currentQuestion, currentAnswer) : false
  const isLastQuestion = visibleIndex >= parsed.questions.length - 1

  const setSelected = (question: AskUserQuestion, optionId: string) => {
    const isMulti = allowMultiple(question)
    setDraft((current) => {
      const existing = current[question.id] ?? { selectedOptionIds: [], customText: '' }
      const selectedOptionIds = isMulti
        ? existing.selectedOptionIds.includes(optionId)
          ? existing.selectedOptionIds.filter((item) => item !== optionId)
          : [...existing.selectedOptionIds, optionId]
        : [optionId]
      return {
        ...current,
        [question.id]: {
          ...existing,
          selectedOptionIds,
        },
      }
    })
    if (!isMulti) {
      const questionIndex = parsed.questions.findIndex((item) => item.id === question.id)
      if (questionIndex >= 0 && questionIndex < parsed.questions.length - 1) {
        setCurrentIndex(questionIndex + 1)
      }
    }
  }

  const setCustomText = (questionId: string, customText: string) => {
    setDraft((current) => ({
      ...current,
      [questionId]: {
        selectedOptionIds: current[questionId]?.selectedOptionIds ?? [],
        customText,
      },
    }))
  }

  const goPrevious = () => {
    setCurrentIndex((index) => Math.max(0, index - 1))
  }

  const goNext = () => {
    setCurrentIndex((index) => Math.min(parsed.questions.length - 1, index + 1))
  }

  const submit = async (skipped: boolean) => {
    const toolCallId = toolCall.toolCallId || toolCall.id
    if (!toolCallId || submitting) return
    setSubmitting(true)
    setSubmitError('')
    try {
      const answers = Object.fromEntries(parsed.questions.map((question) => {
        const answer = draft[question.id] ?? { selectedOptionIds: [], customText: '' }
        const customText = allowCustom(question) ? answer.customText.trim() : ''
        return [
          question.id,
          {
            selected_option_ids: answer.selectedOptionIds,
            custom_text: customText || null,
          },
        ]
      }))
      await api.chatSubmitUserChoice(toolCallId, answers, skipped)
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : String(error))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="not-prose my-2 w-full max-w-[min(100%,32rem)] rounded-md border border-neutral-200/70 bg-white/90 px-3 py-2.5 text-[12px] leading-5 text-neutral-700 shadow-[0_10px_28px_-26px_rgba(0,0,0,0.45),0_1px_2px_rgba(0,0,0,0.035)] dark:border-neutral-700/70 dark:bg-neutral-900/80 dark:text-neutral-200">
      <div className="flex items-center gap-2">
        <MessageSquareMore size={12} strokeWidth={1.9} className="shrink-0 text-neutral-400 dark:text-neutral-500" />
        <div className="min-w-0 flex-1 truncate text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
          {compactText(parsed.title, 96)}
        </div>
        <span className={`shrink-0 text-[10.5px] ${phaseTone(parsed.phase)}`}>
          {phaseLabel(parsed.phase)}
        </span>
      </div>

      {awaiting && currentQuestion ? (
        <>
          <div className="mt-2 flex items-center gap-2">
            <div className="flex flex-1 items-center gap-1">
              {parsed.questions.map((question, index) => {
                const answered = draftHasAnswer(question, draft[question.id])
                const active = index === visibleIndex
                return (
                  <button
                    key={question.id}
                    type="button"
                    onMouseDown={preventMouseFocus}
                    onClick={() => setCurrentIndex(index)}
                    className={`h-1 rounded-full transition-[width,background-color] duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${
                      active
                        ? 'w-6 bg-neutral-900 dark:bg-neutral-100'
                        : answered
                          ? 'w-2.5 bg-neutral-400/80 dark:bg-neutral-500'
                          : 'w-2.5 bg-neutral-200 dark:bg-neutral-700'
                    }`}
                    aria-label={`问题 ${index + 1}`}
                  />
                )
              })}
            </div>
            <span className="shrink-0 text-[10.5px] tabular-nums text-neutral-400 dark:text-neutral-500">
              {visibleIndex + 1}/{parsed.questions.length} · 已答 {answeredCount}
            </span>
          </div>

          <div key={visibleIndex} className="chat-motion-fade mt-2">
            <div className="mb-1.5 flex items-start justify-between gap-3">
              <div className="min-w-0 flex-1 text-[13px] font-medium leading-5 text-neutral-950 dark:text-neutral-50">
                {currentQuestion.prompt}
              </div>
              <span className="mt-0.5 shrink-0 rounded-full bg-neutral-100 px-1.5 py-0.5 text-[10px] leading-4 text-neutral-400 dark:bg-neutral-800 dark:text-neutral-500">
                {allowMultiple(currentQuestion) ? '多选' : '单选'}
              </span>
            </div>

            <div ref={optionsScrollRef} className="chat-popover-scroll max-h-[8.5rem] overflow-y-auto pr-1">
              <div className={currentQuestion.options.some((option) => Boolean(option.description))
                ? 'grid grid-cols-1 gap-1.5 sm:grid-cols-2'
                : 'flex flex-wrap gap-1.5'}
              >
                {currentQuestion.options.map((option) => {
                  const isMulti = allowMultiple(currentQuestion)
                  const hasDescriptions = currentQuestion.options.some((item) => Boolean(item.description))
                  const selected = currentAnswer.selectedOptionIds.includes(option.id)
                  const Icon = isMulti
                    ? selected ? CheckSquare2 : Square
                    : selected ? CheckCircle2 : Circle
                  return (
                    <button
                      key={option.id}
                      type="button"
                      onMouseDown={preventMouseFocus}
                      onClick={() => setSelected(currentQuestion, option.id)}
                      className={`${
                        hasDescriptions
                          ? 'flex min-h-9 items-start gap-2 rounded-md border px-2 py-1.5'
                          : 'inline-flex min-h-7 max-w-full items-center gap-1.5 rounded-full border px-2.5 py-1'
                      } text-left transition-colors ${
                        selected
                          ? 'border-neutral-900/35 bg-neutral-900/[0.04] text-neutral-950 dark:border-neutral-100/30 dark:bg-white/[0.1] dark:text-neutral-50'
                          : 'border-neutral-200/80 bg-transparent text-neutral-700 hover:border-neutral-300 hover:bg-neutral-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:border-neutral-600 dark:hover:bg-neutral-800/60'
                      }`}
                    >
                      <Icon
                        size={hasDescriptions ? 14 : 12}
                        strokeWidth={2}
                        className={`${hasDescriptions ? 'mt-0.5 shrink-0' : 'shrink-0'}${selected ? ' chat-motion-pop' : ''}`}
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[12.5px] font-medium">{option.label}</span>
                        {option.description && (
                          <span className="line-clamp-1 block text-[10.5px] leading-4 text-neutral-500 dark:text-neutral-400">
                            {option.description}
                          </span>
                        )}
                      </span>
                    </button>
                  )
                })}
              </div>
            </div>

            {allowCustom(currentQuestion) && (
              <input
                value={currentAnswer.customText}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => setCustomText(currentQuestion.id, event.target.value)}
                placeholder="其他"
                className="mt-1.5 h-7 w-full rounded-md border border-neutral-200/80 bg-transparent px-2.5 text-[12px] outline-none transition-colors placeholder:text-neutral-400 focus:border-neutral-400 focus:bg-white dark:border-neutral-700 dark:placeholder:text-neutral-500 dark:focus:border-neutral-500 dark:focus:bg-neutral-950"
              />
            )}
          </div>
        </>
      ) : (
        <div className="chat-popover-scroll mt-2 max-h-28 overflow-y-auto">
          <div className="space-y-1.5">
            {parsed.questions.map((question) => (
              <div key={question.id} className="flex items-start gap-2">
                <span className="min-w-0 flex-1 truncate text-[11.5px] text-neutral-500 dark:text-neutral-400">
                  {question.prompt}
                </span>
                <span className="max-w-[55%] truncate rounded-full bg-neutral-100 px-2 py-0.5 text-[11px] text-neutral-700 dark:bg-neutral-800 dark:text-neutral-300">
                  {readonlySummary(question, parsed.answers[question.id])}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      {submitError && (
        <div className="mt-2 flex items-start gap-1.5 text-[11px] leading-4 text-red-500">
          <XCircle size={13} strokeWidth={1.9} className="mt-0.5 shrink-0" />
          <span>{compactText(submitError, 180)}</span>
        </div>
      )}

      {awaiting && (
        <div className="mt-2 flex items-center justify-between gap-2">
          <Button
            variant="ghost"
            size="sm"
            onMouseDown={preventMouseFocus}
            onClick={goPrevious}
            disabled={visibleIndex === 0 || submitting}
          >
            <ArrowLeft size={12} strokeWidth={1.9} />
            上一题
          </Button>
          <div className="flex items-center gap-1.5">
            {!isLastQuestion && (
              <Button
                variant="ghost"
                size="sm"
                onMouseDown={preventMouseFocus}
                onClick={goNext}
                disabled={!currentAnswered || submitting}
              >
                下一题
                <ArrowRight size={12} strokeWidth={1.9} />
              </Button>
            )}
          <Button
            variant="ghost"
            size="sm"
            onMouseDown={preventMouseFocus}
            onClick={() => void submit(true)}
            disabled={submitting}
          >
            <SkipForward size={12} strokeWidth={1.9} />
            跳过
          </Button>
          <Button
            variant="primary"
            size="md"
            onMouseDown={preventMouseFocus}
            onClick={() => void submit(false)}
            disabled={!allAnswered || submitting}
          >
            {submitting ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} strokeWidth={1.9} />}
            提交
          </Button>
          </div>
        </div>
      )}
    </div>
  )
}
