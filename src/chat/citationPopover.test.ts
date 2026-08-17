import { describe, expect, it } from 'vitest'
import { citationPopoverPosition } from './citationPopover'

describe('citationPopoverPosition', () => {
  it('clamps a right-edge citation inside the viewport', () => {
    expect(citationPopoverPosition(
      { left: 900, top: 200, right: 920, bottom: 220 },
      { width: 320, height: 180 },
      { width: 1000, height: 800 },
    )).toEqual({ left: 672, top: 224 })
  })

  it('opens above the citation when the card would cross the bottom edge', () => {
    expect(citationPopoverPosition(
      { left: 300, top: 700, right: 320, bottom: 720 },
      { width: 320, height: 240 },
      { width: 1000, height: 800 },
    )).toEqual({ left: 300, top: 456 })
  })

  it('keeps oversized cards inside the viewport margins', () => {
    expect(citationPopoverPosition(
      { left: -20, top: 4, right: 0, bottom: 20 },
      { width: 984, height: 784 },
      { width: 1000, height: 800 },
    )).toEqual({ left: 8, top: 8 })
  })
})
