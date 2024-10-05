The following is a conversation between a human and an AI agent, 
who is taking actions on behalf of the human.

Messages sent by the human are marked with the "user" role.
Messages sent by the AI are marked with the "assistant" role.
When the AI requests a tool be called, there will be a tool_use and a corresponding tool_result.

These are the tools available to the agent:
{% for tool in exchange.tools %}
{{tool.name}}: {{tool.description}}{% endfor %}

# Messages
{% for message in messages %}
{{message.summary}}
{% endfor %}

# Instructions

Summarize the conversation so far. Highlight the relevant details, making sure to include any
relevant tool use and result content that is important for the conversation.

To preserve space, you can omit some details:
- File content details from tools like read and write file: they will be included separately
- Errors that have already been resolved: provide only the current working details

Make sure to emphasize the most recent request from the human at the end of your summary.
