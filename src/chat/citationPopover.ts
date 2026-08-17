export interface CitationPopoverRect {
  left: number
  top: number
  right: number
  bottom: number
}

export interface CitationPopoverSize {
  width: number
  height: number
}

export interface CitationPopoverViewport {
  width: number
  height: number
}

export interface CitationPopoverPosition {
  left: number
  top: number
}

const VIEWPORT_MARGIN = 8
const ANCHOR_GAP = 4

export function citationPopoverPosition(
  anchor: CitationPopoverRect,
  popover: CitationPopoverSize,
  viewport: CitationPopoverViewport,
): CitationPopoverPosition {
  const maxLeft = Math.max(VIEWPORT_MARGIN, viewport.width - popover.width - VIEWPORT_MARGIN)
  const left = Math.min(maxLeft, Math.max(VIEWPORT_MARGIN, anchor.left))

  const below = anchor.bottom + ANCHOR_GAP
  const above = anchor.top - popover.height - ANCHOR_GAP
  const preferredTop = below + popover.height <= viewport.height - VIEWPORT_MARGIN ? below : above
  const maxTop = Math.max(VIEWPORT_MARGIN, viewport.height - popover.height - VIEWPORT_MARGIN)
  const top = Math.min(maxTop, Math.max(VIEWPORT_MARGIN, preferredTop))

  return { left, top }
}
