## Bash Tool Execution Rules

- For named services (when `name` is set), NEVER pass `async` or top-level `timeout`.
- Use `ready.timeout` exclusively for service readiness deadlines.
- For background jobs, use `async: true` without `name`.
- For finite commands, use top-level `timeout` without `name`.
- Before every Bash tool call, verify that parameters are compatible with the selected execution mode.
- If this validation error occurs, correct the parameters and retry automatically.

回答は日本語で生成．
