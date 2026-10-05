
## Newly completed provider integration

Non-GPT-5 OpenAI-compatible Chat Completions requests now include function-tool schemas for the built-ins. When the provider returns a function call, RIGA validates its JSON arguments, executes it through the workspace-scoped registry, appends the result as a tool message, and continues the model run for at most six tool turns. This keeps write and shell gates enforced in one place.

GPT-5 models continue to use the Responses API path. Responses-native function-call continuation is the next compatibility step because its function-call and function-call-output envelopes differ from Chat Completions.
