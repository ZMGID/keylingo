let parsedCharacterCount = 0

export function recordLiveMarkdownParsedCharacters(count: number): void {
  parsedCharacterCount += count
}

export function resetLiveMarkdownDiagnostics(): void {
  parsedCharacterCount = 0
}

export function getLiveMarkdownParsedCharacterCount(): number {
  return parsedCharacterCount
}
