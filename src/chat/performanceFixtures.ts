import type { ChatMessage, ChatToolArtifact, ToolCallRecord } from './types'

export type ChatPerformanceFixtureId = 'F1' | 'F2' | 'F3' | 'F4'

export type ChatPerformanceFixture = {
  id: ChatPerformanceFixtureId
  title: string
  messages: ChatMessage[]
  streamingContent?: string
}

const FIXTURE_TIMESTAMP = 1_700_000_000

function message(id: string, role: ChatMessage['role'], content: string, index: number): ChatMessage {
  return { id, role, content, timestamp: FIXTURE_TIMESTAMP + index }
}

function pair(index: number, content: string): ChatMessage[] {
  return [
    message(`f-msg-${index}-user`, 'user', `Question ${index}: summarize this fixture row.`, index * 2),
    message(`f-msg-${index}-assistant`, 'assistant', content, index * 2 + 1),
  ]
}

function codeBlock(index: number): string {
  return `\n\n\`\`\`typescript\nexport function fixture${index}(input: string): string {\n  return input.trim().toUpperCase()\n}\n\`\`\``
}

const imageArtifact: ChatToolArtifact = {
  id: 'fixture-image',
  name: 'fixture.png',
  mime_type: 'image/png',
  data_url: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB',
}

const toolCall: ToolCallRecord = {
  id: 'fixture-tool-call',
  name: 'fixture_tool',
  status: 'success',
  result_preview: 'Generated fixture result',
  artifacts: [imageArtifact],
}

function makeF1(): ChatMessage[] {
  return Array.from({ length: 100 }, (_, index) => pair(index, `A normal fixture answer with stable text content. Row ${index}.`)).flat()
}

function makeF2(): ChatMessage[] {
  return Array.from({ length: 20 }, (_, index) => pair(
    index,
    `A code-heavy answer ${index}.${Array.from({ length: 10 }, (_, block) => codeBlock(index * 10 + block)).join('')}`,
  )).flat()
}

function makeF3(): ChatMessage[] {
  return Array.from({ length: 12 }, (_, index) => pair(
    index,
    [
      `Structured answer ${index}.`,
      '\n\n| name | value |\n| --- | ---: |\n| alpha | 42 |\n| beta | 84 |',
      '\n\nInline math $x^2 + y^2 = z^2$.',
      '\n\n```mermaid\ngraph TD\n  A[Input] --> B[Output]\n```',
      '\n\n![fixture](fixture.png)',
    ].join(''),
  ).map((item) => item.role === 'assistant'
    ? { ...item, artifacts: [imageArtifact], tool_calls: [toolCall] }
    : item)).flat()
}

function makeF4(): { messages: ChatMessage[]; streamingContent: string } {
  const unit = 'Streaming fixture line with a stable token budget.\n'
  const streamingContent = unit.repeat(Math.ceil(20_000 / unit.length)).slice(0, 20_000)
  return {
    messages: [message('f4-user', 'user', 'Generate a very long streaming answer.', 0)],
    streamingContent,
  }
}

export function createChatPerformanceFixture(id: ChatPerformanceFixtureId): ChatPerformanceFixture {
  if (id === 'F1') return { id, title: '200 ordinary text rows', messages: makeF1() }
  if (id === 'F2') return { id, title: '20 answers with 200 code blocks', messages: makeF2() }
  if (id === 'F3') return { id, title: 'Structured content mix', messages: makeF3() }
  const fixture = makeF4()
  return { id, title: '20k character streaming answer', ...fixture }
}

export function summarizeChatPerformanceFixture(fixture: ChatPerformanceFixture) {
  const assistantMessages = fixture.messages.filter((item) => item.role === 'assistant')
  const content = [
    ...fixture.messages.map((item) => item.content),
    fixture.streamingContent ?? '',
  ].join('\n')
  return {
    id: fixture.id,
    messageCount: fixture.messages.length,
    assistantCount: assistantMessages.length,
    codeBlockCount: (content.match(/```/g) ?? []).length / 2,
    markdownTableCount: (content.match(/^\|.+\|$/gm) ?? []).length / 3,
    mermaidCount: (content.match(/```mermaid\b/g) ?? []).length,
    imageCount: assistantMessages.reduce((count, item) => count + (item.artifacts?.length ?? 0), 0),
    toolCallCount: assistantMessages.reduce((count, item) => count + (item.tool_calls?.length ?? 0), 0),
    streamingLength: fixture.streamingContent?.length ?? 0,
  }
}
