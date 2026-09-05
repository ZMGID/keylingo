// A demonstration policy, not a filesystem security boundary. Never edits files.
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => { input += chunk; });
process.stdin.on('end', () => {
  const event = JSON.parse(input);
  const path = event.tool_input?.path;
  if (['write', 'edit'].includes(event.tool_name) && typeof path === 'string'
      && path.replaceAll('\\', '/').split('/').includes('protected')) {
    process.stdout.write(JSON.stringify({
      hookSpecificOutput: {
        hookEventName: 'PreToolUse',
        permissionDecision: 'deny',
        permissionDecisionReason: '示例规则：请勿修改 protected 目录中的文件。'
      }
    }));
  }
});
