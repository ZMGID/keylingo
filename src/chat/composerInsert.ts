// 消息区「添加到聊天」→ 输入框 的单监听信道。
// 同一时刻只有一个活跃 composer（InputBar），故单监听足够。
// ponytail: single listener; 若将来同屏多 composer 再改成 Set。
type Listener = (text: string) => void

let listener: Listener | null = null

export function onComposerInsert(cb: Listener): () => void {
  listener = cb
  return () => {
    if (listener === cb) listener = null
  }
}

export function insertIntoComposer(text: string): void {
  listener?.(text)
}

// 文本直插信道（Right Dock「插入 @ 引用」等）：与上面的引用卡片信道并列，
// 区别是文本直接进输入框正文而不是挂成引用卡片。同样单监听。
let textListener: Listener | null = null

export function onComposerTextInsert(cb: Listener): () => void {
  textListener = cb
  return () => {
    if (textListener === cb) textListener = null
  }
}

export function insertTextIntoComposer(text: string): void {
  textListener?.(text)
}
