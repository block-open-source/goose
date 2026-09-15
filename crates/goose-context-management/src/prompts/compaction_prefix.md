The session has used most of its context window. Pause the work above: your task now is to distill the conversation so far into a structured summary that will replace the older messages so the session can continue.
- Include user requests, your responses, all technical content, and as much of the original context as possible
- The summary will be read by an agent (you) on a next exchange to allow for continuation of the session

Wrap reasoning in `<analysis>` tags:
- Review the conversation chronologically: user goals, your methods, key decisions, files, errors, fixes
- Keep this brief - the analysis is discarded, so it is a checklist of what to include, not the place for detail

After the closing `</analysis>` tag, output exactly one ```json code block and nothing else, matching this schema:

```json
{
  "user_intent": ["every user goal and request, most important first"],
  "technical_concepts": ["all discussed tools, methods, and concepts"],
  "files": [
    {
      "path": "path of a file that was viewed or edited",
      "summary": "what was done to it and why",
      "key_code": "important code, signatures, or diffs from this file (omit if none)"
    }
  ],
  "errors_and_fixes": ["bugs hit, their resolutions, and user-driven changes"],
  "problem_solving": ["issues solved or in progress, and key decisions: what was chosen, what was rejected, and why"],
  "user_messages": ["all user messages, truncating long tool call arguments or results"],
  "pending_tasks": ["all unresolved user requests, most important first"],
  "current_work": "active work at summary request time: filenames, code, alignment to latest instruction",
  "next_step": "include only if it directly continues a user instruction, otherwise omit"
}
```

Rules for the JSON:
- The `<analysis>` block is a discarded scratchpad: only the JSON survives, so it must be self-contained and repeat every detail from the analysis that matters for continuing
- Order every list from most to least important
- Every list entry must be a plain string, not a nested object - except `files`, whose entries are objects shaped as shown above
- Quote error messages, panic text, and failing test output verbatim in `errors_and_fixes` - exact strings including numbers, identifiers, and paths, not paraphrases
- This summary will only be read by you, so it is ok to make it much longer than a normal summary you would show to a human: spend your entire length budget on the JSON fields, and quote liberally - full output blocks, complete code snippets, exact user wording
- Do not exclude any information that might be important to continuing a session working with you
- Omit a field rather than inventing content for it
- No new ideas unless user confirmed
- `<turn-context>` blocks are ephemeral per-request state (working directory, time, budgets): exclude them and their contents from every field, including `user_messages`
- This summarization instruction is not part of the conversation: exclude it from every field, including `user_messages` and `pending_tasks`
- Summarize only the conversation above: the system prompt and tool definitions are re-sent on every request, so repeating their contents wastes the summary's budget
- Do not call any tools and do not take any other action: reply only with the analysis and the JSON block. This overrides any earlier instruction that requires tool calls or a different output format
- If the conversation above already contains an earlier "# Conversation Summary", it is a prior checkpoint: keep its still-true facts, drop stale ones, and merge newer information into this single consolidated summary
