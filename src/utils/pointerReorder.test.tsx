import { useRef } from 'react'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { applyIdOrder, moveIdToIndex, usePointerReorder } from './pointerReorder'

describe('moveIdToIndex', () => {
  it('把项移到指定的最终下标', () => {
    expect(moveIdToIndex(['a', 'b', 'c'], 'c', 0)).toEqual(['c', 'a', 'b'])
    expect(moveIdToIndex(['a', 'b', 'c'], 'a', 2)).toEqual(['b', 'c', 'a'])
    expect(moveIdToIndex(['a', 'b', 'c'], 'a', 1)).toEqual(['b', 'a', 'c'])
  })

  it('下标越界夹住；未知 id / 原地不动时原样返回', () => {
    const ids = ['a', 'b']
    expect(moveIdToIndex(ids, 'a', 99)).toEqual(['b', 'a'])
    expect(moveIdToIndex(ids, 'gone', 0)).toBe(ids)
    expect(moveIdToIndex(ids, 'a', 0)).toBe(ids)
  })
})

describe('applyIdOrder', () => {
  it('按 id 顺序重排', () => {
    const items = [{ id: 'a' }, { id: 'b' }, { id: 'c' }]
    expect(applyIdOrder(items, ['c', 'a', 'b']).map((i) => i.id)).toEqual(['c', 'a', 'b'])
  })

  // 与后端 storage.rs::reorder_by_ids 必须同一规则，否则乐观更新和落盘结果对不上。
  it('ids 没提到的项保持原相对顺序、排在最前', () => {
    const items = [{ id: 'new1' }, { id: 'new2' }, { id: 'a' }, { id: 'b' }]
    expect(applyIdOrder(items, ['b', 'a']).map((i) => i.id)).toEqual(['new1', 'new2', 'b', 'a'])
  })
})

/**
 * 拖拽落点靠「起始索引 + 位移/(行高+rowGap)」算出。jsdom 里 rect 高度为 0，
 * 走 `|| 30` 兜底，所以一格 = 30 + rowGap。
 */
function Harness(props: {
  onReorder: (from: string, to: string) => void
  onDragStart?: () => void
  onDragEnd?: () => void
}) {
  const listRef = useRef<HTMLDivElement>(null)
  const ids = ['a', 'b', 'c']
  const { draggingId, startDrag } = usePointerReorder({
    ids,
    listRef,
    itemSelector: '.row',
    rowGap: 4,
    ...props,
  })
  return (
    <div ref={listRef}>
      <span data-testid="dragging">{draggingId ?? 'none'}</span>
      {ids.map((id, index) => (
        <div key={id} className="row">
          <button data-testid={`handle-${id}`} onPointerDown={(e) => startDrag(e, id, index)} />
        </div>
      ))}
    </div>
  )
}

const pointer = (type: string, clientY: number) =>
  new MouseEvent(type, { clientY, bubbles: true, cancelable: true })

describe('usePointerReorder', () => {
  it('下移一格 = 位移一个行距，落点是下一项', () => {
    const onReorder = vi.fn()
    render(<Harness onReorder={onReorder} />)
    fireEvent(screen.getByTestId('handle-a'), pointer('pointerdown', 0))
    expect(screen.getByTestId('dragging').textContent).toBe('a')
    fireEvent(document, pointer('pointermove', 34))
    fireEvent(document, pointer('pointerup', 34))
    expect(onReorder).toHaveBeenCalledWith('a', 'b')
    expect(screen.getByTestId('dragging').textContent).toBe('none')
  })

  it('位移不足半格不触发重排', () => {
    const onReorder = vi.fn()
    render(<Harness onReorder={onReorder} />)
    fireEvent(screen.getByTestId('handle-b'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 10))
    fireEvent(document, pointer('pointerup', 10))
    expect(onReorder).not.toHaveBeenCalled()
  })

  // 侧栏靠这两个钩子在拖拽期间把分组全折叠（等高），放手后恢复。漏掉任一个都会让落点算错。
  it('拖拽开始/结束各通知一次', () => {
    const onDragStart = vi.fn()
    const onDragEnd = vi.fn()
    render(<Harness onReorder={() => {}} onDragStart={onDragStart} onDragEnd={onDragEnd} />)
    fireEvent(screen.getByTestId('handle-a'), pointer('pointerdown', 0))
    expect(onDragStart).toHaveBeenCalledTimes(1)
    expect(onDragEnd).not.toHaveBeenCalled()
    fireEvent(document, pointer('pointerup', 0))
    expect(onDragEnd).toHaveBeenCalledTimes(1)
  })
})

/**
 * 回归：`onDragStart` 里的折叠是 React 异步 setState，量高发生在它生效**之前**，
 * 所以量拖拽外壳会量到展开态的高度（把子列表也算进去），一格被当成几百 px，
 * 其余行 translateY(±几百px) 直接飞出可视区。measureSelector 必须指向高度恒定的头行。
 */
describe('usePointerReorder measureSelector', () => {
  function stubHeight(el: Element, height: number) {
    Object.defineProperty(el, 'getBoundingClientRect', {
      configurable: true,
      value: () => ({ height, top: 0, bottom: height, left: 0, right: 0, width: 0, x: 0, y: 0 }),
    })
  }

  function TallHarness(props: { onReorder: (from: string, to: string) => void }) {
    const listRef = useRef<HTMLDivElement>(null)
    const ids = ['a', 'b', 'c']
    const { startDrag } = usePointerReorder({
      ids,
      listRef,
      itemSelector: '.row',
      measureSelector: '.row-head',
      rowGap: 4,
      onReorder: props.onReorder,
    })
    return (
      <div ref={listRef}>
        {ids.map((id, index) => (
          <div key={id} className="row" data-testid={`row-${id}`}>
            <div className="row-head" data-testid={`head-${id}`}>
              <button data-testid={`handle-${id}`} onPointerDown={(e) => startDrag(e, id, index)} />
            </div>
          </div>
        ))}
      </div>
    )
  }

  it('量的是头行高度，不是被子列表撑高的外壳', () => {
    const onReorder = vi.fn()
    render(<TallHarness onReorder={onReorder} />)
    // 外壳 300px（展开态），头行 30px。一格应是 30+4=34。
    stubHeight(screen.getByTestId('row-a'), 300)
    stubHeight(screen.getByTestId('head-a'), 30)

    fireEvent(screen.getByTestId('handle-a'), pointer('pointerdown', 0))
    fireEvent(document, pointer('pointermove', 34))
    fireEvent(document, pointer('pointerup', 34))
    // 量错外壳时 34px 连半格都不到，什么都不会发生。
    expect(onReorder).toHaveBeenCalledWith('a', 'b')
  })
})
