import { useRef } from 'react'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { useInsertionReorder } from './insertionReorder'

/** jsdom 不排版，所以行位置全部显式伪造：每行高 30，依次排在 0/30/60。 */
function stub(el: HTMLElement, top: number, height: number) {
  Object.defineProperty(el, 'offsetTop', { configurable: true, value: top })
  Object.defineProperty(el, 'offsetHeight', { configurable: true, value: height })
}

function Harness({ onDrop }: { onDrop: (id: string, to: number, scope: string) => void }) {
  const listRef = useRef<HTMLDivElement>(null)
  const ids = ['a', 'b', 'c']
  const { draggingId, lineTop, ghostPos, startDrag } = useInsertionReorder({
    listRef,
    rowSelector: '.row',
    onDrop,
  })
  return (
    <div ref={listRef}>
      <span data-testid="state">{`${draggingId ?? 'none'}:${lineTop ?? '-'}:${ghostPos ? 'ghost' : '-'}`}</span>
      {ids.map((id) => (
        <div
          key={id}
          className="row"
          data-reorder-id={id}
          data-testid={`row-${id}`}
          onPointerDown={(e) => startDrag(e, id)}
        >
          <button data-testid={`menu-${id}`} data-no-drag />
        </div>
      ))}
    </div>
  )
}

const pointer = (type: string, clientY: number, clientX = 0) =>
  new MouseEvent(type, { clientY, clientX, bubbles: true, cancelable: true, button: 0 })

function setup(onDrop = vi.fn()) {
  const utils = render(<Harness onDrop={onDrop} />)
  stub(screen.getByTestId('row-a'), 0, 30)
  stub(screen.getByTestId('row-b'), 30, 30)
  stub(screen.getByTestId('row-c'), 60, 30)
  return { onDrop, ...utils }
}

const state = () => screen.getByTestId('state').textContent

describe('useInsertionReorder', () => {
  it('位移不过 5px 阈值时压根不进入拖拽（当普通点击处理）', () => {
    const { onDrop } = setup()
    fireEvent(screen.getByTestId('row-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 3))
    expect(state()).toBe('none:-:-')
    fireEvent(document, pointer('pointerup', 3))
    expect(onDrop).not.toHaveBeenCalled()
  })

  it('过阈值后进入拖拽：出现浮起卡片和插入线', () => {
    setup()
    fireEvent(screen.getByTestId('row-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 50))
    // 插入槽 2 → 线在第 1 行（b）的底边 = 30 + 30
    expect(state()).toBe('a:60:ghost')
    fireEvent(document, pointer('pointerup', 50))
    expect(state()).toBe('none:-:-')
  })

  it('拖过下一行的中线才落到它后面', () => {
    const { onDrop } = setup()
    fireEvent(screen.getByTestId('row-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 50))
    fireEvent(document, pointer('pointerup', 50))
    expect(onDrop).toHaveBeenCalledWith('a', 1, '')
  })

  it('拖到最底下落到末位，拖到最上面落到首位', () => {
    const { onDrop } = setup()
    fireEvent(screen.getByTestId('row-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 200))
    fireEvent(document, pointer('pointerup', 200))
    expect(onDrop).toHaveBeenCalledWith('a', 2, '')

    onDrop.mockClear()
    fireEvent(screen.getByTestId('row-c'), pointer('pointerdown', 60))
    fireEvent(document, pointer('pointermove', -100))
    fireEvent(document, pointer('pointerup', -100))
    expect(onDrop).toHaveBeenCalledWith('c', 0, '')
  })

  it('原地放手不触发重排', () => {
    const { onDrop } = setup()
    fireEvent(screen.getByTestId('row-b'), pointer('pointerdown', 40))
    fireEvent(document, pointer('pointermove', 48))
    fireEvent(document, pointer('pointerup', 48))
    expect(onDrop).not.toHaveBeenCalled()
  })

  // 没有把手了，整行可拖 —— 行内的菜单等控件必须挡住，否则点菜单会变成拖拽。
  it('data-no-drag 的控件上按下不触发拖拽', () => {
    const { onDrop } = setup()
    fireEvent(screen.getByTestId('menu-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 50))
    expect(state()).toBe('none:-:-')
    fireEvent(document, pointer('pointerup', 50))
    expect(onDrop).not.toHaveBeenCalled()
  })

  it('真拖过之后吞掉紧随的那次 click（否则放手就把这行选中了）', () => {
    setup()
    const onClick = vi.fn()
    document.addEventListener('click', onClick)
    fireEvent(screen.getByTestId('row-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 50))
    fireEvent(document, pointer('pointerup', 50))
    fireEvent.click(screen.getByTestId('row-a'))
    expect(onClick).not.toHaveBeenCalled()
    document.removeEventListener('click', onClick)
  })
})

/**
 * 侧栏用一个 hook 实例服务所有分组，靠 `data-reorder-scope` 隔离取样范围。
 * 这条挂了就说明多个集/项目同时展开时，行会跨分组互串（落点算到别人家的行上）。
 */
describe('useInsertionReorder 作用域隔离', () => {
  function ScopedHarness({ onDrop }: { onDrop: (id: string, to: number, scope: string) => void }) {
    const listRef = useRef<HTMLDivElement>(null)
    const { startDrag } = useInsertionReorder({ listRef, rowSelector: '.row', onDrop })
    const group = (scope: string, ids: string[]) => (
      <div key={scope} data-reorder-scope={scope}>
        {ids.map((id) => (
          <div
            key={id}
            className="row"
            data-reorder-id={id}
            data-testid={`row-${id}`}
            onPointerDown={(e) => startDrag(e, id)}
          />
        ))}
      </div>
    )
    return (
      <div ref={listRef}>
        {group('g1', ['a', 'b'])}
        {group('g2', ['c', 'd'])}
      </div>
    )
  }

  it('只在本作用域内取样，并把 scopeId 带回来', () => {
    const onDrop = vi.fn()
    render(<ScopedHarness onDrop={onDrop} />)
    // g1 在 0..60，g2 在 60..120。若不隔离，g2 的 c 会看到 4 行、落点算错。
    stub(screen.getByTestId('row-a'), 0, 30)
    stub(screen.getByTestId('row-b'), 30, 30)
    stub(screen.getByTestId('row-c'), 60, 30)
    stub(screen.getByTestId('row-d'), 90, 30)

    fireEvent(screen.getByTestId('row-c'), pointer('pointerdown', 60))
    fireEvent(document, pointer('pointermove', 200))
    fireEvent(document, pointer('pointerup', 200))
    // g2 里只有 c、d 两行，c 拖到底 → 最终下标 1（不是全局的 3）
    expect(onDrop).toHaveBeenCalledWith('c', 1, 'g2')
  })
})
