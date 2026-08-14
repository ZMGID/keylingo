import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from 'react'
import {
  ArrowRight,
  Check,
  ChevronLeft,
  ChevronRight,
  Loader2,
  MessageSquareMore,
  Pencil,
  Send,
  X,
  XCircle,
} from 'lucide-react'
import { api } from '../api/tauri'
import { Button, IconButton } from '../components/Button'
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
  /** `docked` = 吊在输入框上方的那张主面板（用户在这里作答）；
   *  `inline` = 消息流里的痕迹，待答时只留一行「等待你回答」，答完显示答案摘要。
   *  两处同时渲染整张卡会出现两个可作答的副本。 */
  variant?: 'docked' | 'inline'
  /** 作答/跳过成功后通知宿主收起这张面板（后端没有「已答复」事件，见 Chat.tsx 的注释）。 */
  onResolved?: () => void
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
    const multiple = question.allow_multiple === true || question.allowMultiple === true
    const custom = question.allow_custom === true || question.allowCustom === true
    // 内置 ask_user 仍要求至少两个选项；外部 CLI 的纯文本作答靠 allow_custom 放行 0 个选项。
    if (!id || !prompt || (options.length < 2 && !custom)) continue
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

export function AskUserBlock({ toolCall, variant = 'inline', onResolved }: AskUserBlockProps) {
  const parsed = useMemo(() => parseAskUser(toolCall), [toolCall])
  const parsedRef = useRef(parsed)
  parsedRef.current = parsed
  const docked = variant === 'docked'
  const signature = promptSignature(parsed)
  const optionsScrollRef = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const customInputRef = useRef<HTMLInputElement>(null)
  const [draft, setDraft] = useState<Record<string, DraftAnswer>>(() => createDraft(parsed))
  /** 草稿的同步镜像。两个用途，都是「不能等下一次渲染」：
   *  ① 单题单选点一下就提交，要**当场**拿到含这次选择的草稿（`draft` 还是旧值）；
   *  ② 同一批次里连点两下（React 会把它们合并成一次渲染）时，第二下要基于第一下的结果 ——
   *     只读 `draft` 的话第二下会把第一下覆盖掉（多选实测：连点两项只剩后一项）。 */
  const draftRef = useRef(draft)
  const [currentIndex, setCurrentIndex] = useState(() => firstUnansweredIndex(parsed, createDraft(parsed)))
  const submitRef = useRef<(skipped: boolean, draftOverride?: Record<string, DraftAnswer>) => void>(() => {})
  /** 键盘光标（与「已选中」是两回事：光标只是高亮，Enter 才落选）。 */
  const [activeIndex, setActiveIndex] = useState(0)
  const [submitting, setSubmitting] = useState(false)
  const [submitError, setSubmitError] = useState('')
  const questionCount = parsed?.questions.length ?? 0
  const visibleIndex = questionCount > 0 ? Math.min(currentIndex, questionCount - 1) : 0

  // 只在**换了另一条询问**时重置草稿。依赖里刻意不放 `parsed`：它是按 `toolCall` 记忆的，
  // 而调用方常常每次渲染都新建 toolCall 对象（docked 面板就是从事件载荷现拼的），
  // 放进去就变成「每次渲染都清空用户选到一半的答案」。`id + signature` 已经能唯一标定一条询问。
  useEffect(() => {
    const nextDraft = createDraft(parsedRef.current)
    draftRef.current = nextDraft
    setDraft(nextDraft)
    setCurrentIndex(firstUnansweredIndex(parsedRef.current, nextDraft))
    setSubmitError('')
  }, [toolCall.id, signature])

  useLayoutEffect(() => {
    const el = optionsScrollRef.current
    if (el) el.scrollTop = 0
    setActiveIndex(0)
  }, [toolCall.id, visibleIndex])

  // 卡片一出现就把焦点收到选项列表上：这一刻整轮生成都停在这里等答复，键盘直接能用
  // （↑↓ / Enter / 数字键）。与审批卡同一套取舍 —— 那边也是让主按钮 autoFocus，
  // 否则焦点留在输入框里、数字键会被当成正文输入。**只有 docked 那张抢焦点**：
  // 消息流里的痕迹不该把光标从输入框里拽走。
  useEffect(() => {
    if (!docked || parsed?.phase !== 'awaiting') return
    listRef.current?.focus({ preventScroll: true })
  }, [docked, toolCall.id, parsed?.phase])

  if (!parsed) {
    return (
      <div className="not-prose mb-2 inline-flex max-w-full items-center gap-1.5 rounded-md py-0.5 text-[11.5px] leading-5 text-neutral-400 dark:text-neutral-500">
        <MessageSquareMore size={12} strokeWidth={1.9} className="shrink-0" />
        <span className="truncate">等待用户确认</span>
      </div>
    )
  }

  // 消息流里的痕迹：待答时只留一行。整张可作答的卡片吊在输入框上方（docked），
  // 两处都渲染整张会出现两个能点的副本、焦点也会打架。
  if (!docked && parsed.phase === 'awaiting') {
    return (
      <div className="not-prose mb-2 inline-flex max-w-full items-center gap-1.5 rounded-md py-0.5 text-[11.5px] leading-5 text-neutral-400 dark:text-neutral-500">
        <MessageSquareMore size={12} strokeWidth={1.9} className="shrink-0" />
        <span className="truncate">等待你回答（在下方面板里选）</span>
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
  /** 单题（绝大多数情况）不显示进度条与上一题/下一题 —— 一道题的「1/1」和翻页键是纯噪声。 */
  const multiQuestion = parsed.questions.length > 1
  const multiSelect = currentQuestion ? allowMultiple(currentQuestion) : false
  /** 单题单选：点一下就是答案，不再要一次「提交」。多选要攒、多题要翻，那两种才留提交键。 */
  const answerOnPick = !multiQuestion && !multiSelect
  const optionCount = currentQuestion?.options.length ?? 0
  /** 键盘光标可停的行数：选项 +（有自定义时）那一行。 */
  const rowCount = optionCount + (currentQuestion && allowCustom(currentQuestion) ? 1 : 0)

  /** 纯函数：把「这一题选了这个选项」应用到草稿上。抽出来是因为「选中即提交」要**同步**拿到
   *  新草稿 —— `setDraft` 之后 `draft` 还是旧值，直接提交会把空答案发出去。 */
  const draftWithSelection = (
    current: Record<string, DraftAnswer>,
    question: AskUserQuestion,
    optionId: string,
  ): Record<string, DraftAnswer> => {
    const existing = current[question.id] ?? { selectedOptionIds: [], customText: '' }
    const selectedOptionIds = allowMultiple(question)
      ? existing.selectedOptionIds.includes(optionId)
        ? existing.selectedOptionIds.filter((item) => item !== optionId)
        : [...existing.selectedOptionIds, optionId]
      : [optionId]
    return { ...current, [question.id]: { ...existing, selectedOptionIds } }
  }

  /** 点一行选项。单题单选时**直接落答案**（把新草稿同步传下去），不用再点提交。 */
  const pickOption = (question: AskUserQuestion, optionId: string) => {
    const next = draftWithSelection(draftRef.current, question, optionId)
    draftRef.current = next
    setDraft(next)
    if (!allowMultiple(question)) {
      const questionIndex = parsed.questions.findIndex((item) => item.id === question.id)
      if (questionIndex >= 0 && questionIndex < parsed.questions.length - 1) {
        setCurrentIndex(questionIndex + 1)
      }
    }
    if (answerOnPick) submitRef.current(false, next)
  }

  const handleListKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!currentQuestion || submitting) return
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const delta = event.key === 'ArrowDown' ? 1 : -1
      setActiveIndex((index) => {
        const next = Math.min(rowCount - 1, Math.max(0, index + delta))
        if (next === optionCount) customInputRef.current?.focus()
        return next
      })
      return
    }
    if (event.key === 'Enter') {
      event.preventDefault()
      const option = currentQuestion.options[activeIndex]
      if (option) pickOption(currentQuestion, option.id)
      else customInputRef.current?.focus()
      return
    }
    // 数字键直选（与审批卡的 1/2/3 同一套肌肉记忆）。
    const digit = Number(event.key)
    if (Number.isInteger(digit) && digit >= 1 && digit <= optionCount) {
      event.preventDefault()
      setActiveIndex(digit - 1)
      pickOption(currentQuestion, currentQuestion.options[digit - 1].id)
    }
  }

  const setCustomText = (questionId: string, customText: string) => {
    const next = {
      ...draftRef.current,
      [questionId]: {
        selectedOptionIds: draftRef.current[questionId]?.selectedOptionIds ?? [],
        customText,
      },
    }
    draftRef.current = next
    setDraft(next)
  }

  const goPrevious = () => {
    setCurrentIndex((index) => Math.max(0, index - 1))
  }

  const goNext = () => {
    setCurrentIndex((index) => Math.min(parsed.questions.length - 1, index + 1))
  }

  const submit = async (skipped: boolean, draftOverride?: Record<string, DraftAnswer>) => {
    const toolCallId = toolCall.toolCallId || toolCall.id
    if (!toolCallId || submitting) return
    // 用镜像而不是 `draft`：自定义输入框按 Enter 时，那一次 onChange 的值还没进渲染。
    const source = draftOverride ?? draftRef.current
    setSubmitting(true)
    setSubmitError('')
    try {
      const answers = Object.fromEntries(parsed.questions.map((question) => {
        const answer = source[question.id] ?? { selectedOptionIds: [], customText: '' }
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
      // 后端没有「已答复」事件（`resolve_user_prompt` 只清重放快照），所以由这里通知宿主
      // 收起面板。答复成功才收 —— 失败要把错误留在面板上让用户重试。
      onResolved?.()
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : String(error))
    } finally {
      setSubmitting(false)
    }
  }

  submitRef.current = (skipped, draftOverride) => { void submit(skipped, draftOverride) }

  return (
    <div className={`not-prose w-full overflow-hidden rounded-2xl border border-neutral-200/70 bg-white/95 text-[12px] leading-5 text-neutral-700 dark:border-neutral-700/70 dark:bg-neutral-900/85 dark:text-neutral-200 ${
      // 只有吊在输入框上方那张才有投影（它是浮在对话之上的层）。消息流里的那块是**内容**，
      // 带投影会在浅灰底上糊出一条脏影子 —— 与旁边其它工具卡（无投影）也不是一套。
      docked
        ? 'shadow-[0_14px_36px_-28px_rgba(0,0,0,0.5),0_1px_2px_rgba(0,0,0,0.035)]'
        : 'my-2 max-w-[min(100%,38rem)]'
    }`}>
      {/* 标题行：问题本身当标题（原来是「需要确认」当标题、问题降到正文，主次颠倒）。
          右侧是翻页 + 跳过，同参考图的 `‹ 1 of 4 ›  ×`。 */}
      <div className="flex items-start gap-3 px-3.5 pt-3 pb-2">
        <div className="min-w-0 flex-1 text-[14px] font-semibold leading-6 text-neutral-950 dark:text-neutral-50">
          {awaiting && currentQuestion ? currentQuestion.prompt : compactText(parsed.title, 96)}
          {awaiting && multiSelect && (
            <span className="ml-1.5 align-[1px] text-[11px] font-normal text-neutral-400 dark:text-neutral-500">
              可多选
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-0.5 pt-0.5">
          {awaiting && multiQuestion && (
            <>
              <IconButton
                label="上一题"
                size="xs"
                variant="ghost"
                disabled={visibleIndex === 0 || submitting}
                onMouseDown={preventMouseFocus}
                onClick={goPrevious}
              >
                <ChevronLeft size={14} strokeWidth={2} />
              </IconButton>
              <span className="px-0.5 text-[11.5px] tabular-nums text-neutral-500 dark:text-neutral-400">
                {visibleIndex + 1}/{parsed.questions.length}
              </span>
              <IconButton
                label="下一题"
                size="xs"
                variant="ghost"
                disabled={!currentAnswered || isLastQuestion || submitting}
                onMouseDown={preventMouseFocus}
                onClick={goNext}
              >
                <ChevronRight size={14} strokeWidth={2} />
              </IconButton>
            </>
          )}
          {awaiting ? (
            <IconButton
              label="跳过这次询问"
              size="xs"
              variant="ghost"
              disabled={submitting}
              onMouseDown={preventMouseFocus}
              onClick={() => void submit(true)}
            >
              <X size={14} strokeWidth={2} />
            </IconButton>
          ) : (
            <span className={`text-[11px] ${phaseTone(parsed.phase)}`}>{phaseLabel(parsed.phase)}</span>
          )}
        </div>
      </div>

      {awaiting && currentQuestion ? (
        <>
          {/* 选项列表：整行 + 左侧编号 + 细分隔线；键盘光标那行整块高亮并露出 →。
              列表自己是焦点承载者（`tabIndex`），所以 ↑↓ / Enter / 数字键直接可用。 */}
          <div
            ref={listRef}
            role="listbox"
            aria-label={currentQuestion.prompt}
            tabIndex={0}
            onKeyDown={handleListKeyDown}
            className="chat-popover-scroll max-h-[17rem] overflow-y-auto px-2 pb-1 outline-none"
          >
            <div ref={optionsScrollRef} key={visibleIndex} className="chat-motion-fade">
              {currentQuestion.options.map((option, index) => {
                const selected = currentAnswer.selectedOptionIds.includes(option.id)
                const active = index === activeIndex
                return (
                  <button
                    key={option.id}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    onMouseDown={preventMouseFocus}
                    onMouseEnter={() => setActiveIndex(index)}
                    onClick={() => pickOption(currentQuestion, option.id)}
                    className={`flex w-full items-center gap-3 rounded-xl px-1.5 py-2 text-left transition-colors ${
                      // 行之间不画线：当前行的底色已经把「在哪一行」说清楚了，再加分隔线就是两套
                      // 冗余的边界。也别改回「非当前行才加 border-t」—— 那会让行高随 hover 在
                      // 1px 之间变，整张面板（吊在输入框上方）跟着上下跳。尺寸绝不能随 hover 变。
                      active
                        ? 'bg-neutral-100 dark:bg-neutral-800'
                        : 'hover:bg-neutral-50 dark:hover:bg-neutral-800/50'
                    }`}
                  >
                    <span
                      className={`grid size-6 shrink-0 place-items-center rounded-md text-[11.5px] font-medium tabular-nums transition-colors ${
                        selected
                          ? 'bg-neutral-900 text-white dark:bg-neutral-100 dark:text-neutral-900'
                          : 'bg-neutral-100 text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400'
                      }`}
                    >
                      {selected && multiSelect ? <Check size={13} strokeWidth={2.4} /> : index + 1}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block break-words text-[13px] font-medium leading-5 text-neutral-900 dark:text-neutral-100">
                        {option.label}
                      </span>
                      {option.description && (
                        <span className="mt-0.5 line-clamp-2 block break-words text-[11.5px] leading-4 text-neutral-500 dark:text-neutral-400">
                          {option.description}
                        </span>
                      )}
                    </span>
                    {/* 箭头**始终占位**，只切换可见性：只在当前行渲染的话，它会挤掉标签的
                        可用宽度、长标签换行 ⇒ 行高变 ⇒ 鼠标一划整张面板就跳。 */}
                    <ArrowRight
                      size={14}
                      strokeWidth={2}
                      className={`shrink-0 text-neutral-400 transition-opacity dark:text-neutral-500 ${
                        active ? 'opacity-100' : 'opacity-0'
                      }`}
                    />
                  </button>
                )
              })}

              {allowCustom(currentQuestion) && (
                <div
                  className={`flex items-center gap-3 rounded-xl px-1.5 py-1.5 ${
                    // 同上：不画分隔线。
                    activeIndex === optionCount ? 'bg-neutral-100 dark:bg-neutral-800' : ''
                  }`}
                >
                  <span className="grid size-6 shrink-0 place-items-center rounded-md bg-neutral-100 text-neutral-400 dark:bg-neutral-800 dark:text-neutral-500">
                    <Pencil size={12} strokeWidth={2} />
                  </span>
                  <input
                    ref={customInputRef}
                    value={currentAnswer.customText}
                    autoCapitalize="off"
                    autoCorrect="off"
                    autoComplete="off"
                    spellCheck={false}
                    onFocus={() => setActiveIndex(optionCount)}
                    onChange={(event) => setCustomText(currentQuestion.id, event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key !== 'Enter' || !event.currentTarget.value.trim()) return
                      event.preventDefault()
                      if (answerOnPick) void submit(false)
                    }}
                    placeholder="自己写一个…"
                    className="min-w-0 flex-1 bg-transparent text-[13px] leading-5 outline-none placeholder:text-neutral-400 dark:placeholder:text-neutral-500"
                  />
                </div>
              )}
            </div>
          </div>

          <div className="flex items-center gap-2 px-3.5 pb-2.5 pt-1.5">
            <span className="min-w-0 flex-1 truncate text-[11px] text-neutral-400 dark:text-neutral-500">
              ↑↓ 切换 · Enter 选择{optionCount > 1 ? ` · 数字键 1–${Math.min(optionCount, 9)} 直选` : ''}
            </span>
            {!answerOnPick && (
              <Button
                variant="primary"
                size="sm"
                onMouseDown={preventMouseFocus}
                onClick={() => void submit(false)}
                disabled={!allAnswered || submitting}
              >
                {submitting ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} strokeWidth={1.9} />}
                提交
              </Button>
            )}
          </div>
        </>
      ) : (
        /* 已作答：问题 + 用户选的答案逐条列出（参考 Claude Code 答完之后那块）。
           答案**不截断、不塞进右侧小药丸** —— 「我当时选了什么」是这块唯一的价值，
           截一半就等于没留下。 */
        <div className="chat-popover-scroll max-h-64 overflow-y-auto px-3.5 pb-3">
          <div className="space-y-2">
            {parsed.questions.map((question) => (
              <div key={question.id}>
                <div className="break-words text-[12.5px] font-medium leading-5 text-neutral-800 dark:text-neutral-100">
                  {question.prompt}
                </div>
                <div className="mt-0.5 break-words text-[12px] leading-5 text-neutral-500 dark:text-neutral-400">
                  {readonlySummary(question, parsed.answers[question.id])}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {submitError && (
        <div className="flex items-start gap-1.5 px-3.5 pb-2.5 text-[11px] leading-4 text-red-500">
          <XCircle size={13} strokeWidth={1.9} className="mt-0.5 shrink-0" />
          <span>{compactText(submitError, 180)}</span>
        </div>
      )}
    </div>
  )
}
