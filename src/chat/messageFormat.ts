/** 回答元信息条的时间戳。完整日期没人逐条读，却把元信息条撑成一句话
 *  （"Aug 6, 2026 at 9:46 AM"）：今天只给时刻，今年内给「月 日, 时刻」，跨年才带年份。
 *  `now` 仅测试注入。 */
export function formatAssistantMessageTime(timestamp: number, now: Date = new Date()): string {
  const date = new Date(timestamp * 1000)
  const time = new Intl.DateTimeFormat('en-US', {
    hour: 'numeric',
    minute: '2-digit',
    hour12: true,
  }).format(date)
  if (date.toDateString() === now.toDateString()) return time
  const day = new Intl.DateTimeFormat('en-US', {
    month: 'short',
    day: 'numeric',
    ...(date.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }),
  }).format(date)
  return `${day}, ${time}`
}
