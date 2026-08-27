/**
 * 模型连接测试的并发池。单独成文件：组件文件里导出常量/函数会破坏 Fast Refresh。
 */

/** 同时打太多模型会卡住 WebView（IPC + 旋转图标 + 网关限流）。 */
export const MODEL_TEST_CONCURRENCY = 10

export async function runPool<T>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<void>,
): Promise<void> {
  let next = 0
  const n = Math.max(1, Math.min(limit, items.length))
  await Promise.all(
    Array.from({ length: n }, async () => {
      let i = next++
      while (i < items.length) {
        await worker(items[i])
        i = next++
      }
    }),
  )
}
