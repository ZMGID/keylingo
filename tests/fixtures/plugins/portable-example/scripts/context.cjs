let input = ''
process.stdin.setEncoding('utf8')
process.stdin.on('data', chunk => { input += chunk })
process.stdin.on('end', () => {
  const event = JSON.parse(input)
  process.stdout.write(JSON.stringify({
    hookSpecificOutput: {
      hookEventName: event.hook_event_name,
      additionalContext: `Portable example loaded for session ${event.session_id}.`,
    },
  }))
})
