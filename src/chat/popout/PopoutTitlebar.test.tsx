/**
 * @vitest-environment jsdom
 */
import { render } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { LangContext } from '../../settings/i18n'
import type { AgentRuntimeConfig } from '../types'
import { PopoutTitlebar } from './PopoutTitlebar'

vi.mock('../RuntimePicker', () => ({
  RuntimePicker: () => <span data-testid="runtime-picker" />,
  ExternalModelSelector: () => <span />,
}))
vi.mock('../ModelSelector', () => ({ ModelSelector: () => <span data-testid="model-selector" /> }))
vi.mock('../ThinkingLevelSelector', () => ({ ThinkingLevelSelector: () => <span /> }))
vi.mock('../PermissionPicker', () => ({ PermissionPicker: () => <span /> }))

const runtime: AgentRuntimeConfig = { kind: 'builtin' }

describe('PopoutTitlebar', () => {
  it('places the conversation title to the right of the model pills', () => {
    const { container } = render(
      <LangContext.Provider value="zh">
        <PopoutTitlebar
          conversation={{
            id: 'c1',
            revision: 1,
            title: '商品图片批量生成工作流搭建',
            provider_id: 'p',
            model: 'm',
            messages: [],
            created_at: 0,
            updated_at: 0,
          }}
          runtime={runtime}
          usesExternalRuntime={false}
          approvalPolicy="readonly_auto_sensitive_confirm"
          onRuntimeChange={() => {}}
          onModelChange={() => {}}
          onExternalModelChange={() => {}}
          onThinkingLevelChange={() => {}}
          onApprovalPolicyChange={() => {}}
        />
      </LangContext.Provider>,
    )
    const pills = container.querySelector('[data-popout-pills]')
    const title = container.querySelector('[data-popout-title]')
    expect(pills).toBeTruthy()
    expect(title?.textContent).toBe('商品图片批量生成工作流搭建')
    expect(Boolean(pills && title && (pills.compareDocumentPosition(title) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true)
  })
})
