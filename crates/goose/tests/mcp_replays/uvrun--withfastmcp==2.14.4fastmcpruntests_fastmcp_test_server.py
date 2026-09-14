STDIN: {"jsonrpc":"2.0","id":0,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"goose-desktop","version":"0.0.0"},"io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}},"roots":{},"sampling":{},"elicitation":{}}}}}
STDERR: warning: The `native-tls` setting is deprecated and will be removed in a future release. Use `system-certs` instead.
STDERR: /Users/jackamadeo/.cache/uv/archive-v0/bp02wML-MrQTiYuR/lib/python3.14/site-packages/fastmcp/server/auth/providers/jwt.py:10: AuthlibDeprecationWarning: authlib.jose module is deprecated, please use joserfc instead.
STDERR: It will be compatible before version 2.0.0.
STDERR:   from authlib.jose import JsonWebKey, JsonWebToken
STDERR: /Users/jackamadeo/.cache/uv/archive-v0/bp02wML-MrQTiYuR/lib/python3.14/site-packages/authlib/integrations/httpx_client/assertion_client.py:5: AuthlibDeprecationWarning: The httpx module is deprecated; please use httpx2 instead.
STDERR:   from ._compat import httpx2
STDERR: 
STDERR: 
STDERR: ╭──────────────────────────────────────────────────────────────────────────────╮
STDERR: │                                                                              │
STDERR: │                                                                              │
STDERR: │                         ▄▀▀ ▄▀█ █▀▀ ▀█▀ █▀▄▀█ █▀▀ █▀█                        │
STDERR: │                         █▀  █▀█ ▄▄█  █  █ ▀ █ █▄▄ █▀▀                        │
STDERR: │                                                                              │
STDERR: │                                                                              │
STDERR: │                                                                              │
STDERR: │                                FastMCP 2.14.4                                │
STDERR: │                            https://gofastmcp.com                             │
STDERR: │                                                                              │
STDERR: │                    🖥  Server:      mymcp                                     │
STDERR: │                    🚀 Deploy free: https://fastmcp.cloud                     │
STDERR: │                                                                              │
STDERR: ╰──────────────────────────────────────────────────────────────────────────────╯
STDERR: ╭──────────────────────────────────────────────────────────────────────────────╮
STDERR: │                          ✨ FastMCP 3.0 is coming!                           │
STDERR: │       Pin `fastmcp < 3` in production, then upgrade when you're ready.       │
STDERR: ╰──────────────────────────────────────────────────────────────────────────────╯
STDERR: ╭──────────────────────────────────────────────────────────────────────────────╮
STDERR: │                          🎉 Update available: 4.0.1                          │
STDERR: │                      Run: pip install --upgrade fastmcp                      │
STDERR: ╰──────────────────────────────────────────────────────────────────────────────╯
STDERR: 
STDERR: 
STDERR: [09/02/26 13:57:17] INFO     Starting MCP server 'mymcp' with     server.py:2506
STDERR:                              transport 'stdio'                                  
STDERR: /Users/jackamadeo/.cache/uv/archive-v0/bp02wML-MrQTiYuR/lib/python3.14/site-packages/redis/asyncio/connection.py:2861: DeprecationWarning: FakeConnection is deprecated. Use FakeAsyncRedisConnection instead
STDERR:   return self.connection_class(**self.connection_kwargs)
STDERR: WARNING:root:Failed to validate request: 31 validation errors for ClientRequest
STDERR: PingRequest.method
STDERR:   Input should be 'ping' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: InitializeRequest.method
STDERR:   Input should be 'initialize' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: InitializeRequest.params.protocolVersion
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: InitializeRequest.params.capabilities
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: InitializeRequest.params.clientInfo
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: CompleteRequest.method
STDERR:   Input should be 'completion/complete' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: CompleteRequest.params.ref
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: CompleteRequest.params.argument
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: SetLevelRequest.method
STDERR:   Input should be 'logging/setLevel' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: SetLevelRequest.params.level
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: GetPromptRequest.method
STDERR:   Input should be 'prompts/get' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: GetPromptRequest.params.name
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: ListPromptsRequest.method
STDERR:   Input should be 'prompts/list' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: ListResourcesRequest.method
STDERR:   Input should be 'resources/list' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: ListResourceTemplatesRequest.method
STDERR:   Input should be 'resources/templates/list' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: ReadResourceRequest.method
STDERR:   Input should be 'resources/read' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: ReadResourceRequest.params.uri
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: SubscribeRequest.method
STDERR:   Input should be 'resources/subscribe' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: SubscribeRequest.params.uri
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: UnsubscribeRequest.method
STDERR:   Input should be 'resources/unsubscribe' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: UnsubscribeRequest.params.uri
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: CallToolRequest.method
STDERR:   Input should be 'tools/call' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: CallToolRequest.params.name
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: ListToolsRequest.method
STDERR:   Input should be 'tools/list' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: GetTaskRequest.method
STDERR:   Input should be 'tasks/get' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: GetTaskRequest.params.taskId
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: GetTaskPayloadRequest.method
STDERR:   Input should be 'tasks/result' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: GetTaskPayloadRequest.params.taskId
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDERR: ListTasksRequest.method
STDERR:   Input should be 'tasks/list' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: CancelTaskRequest.method
STDERR:   Input should be 'tasks/cancel' [type=literal_error, input_value='server/discover', input_type=str]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/literal_error
STDERR: CancelTaskRequest.params.taskId
STDERR:   Field required [type=missing, input_value={'_meta': {'io.modelconte...{}, 'elicitation': {}}}}, input_type=dict]
STDERR:     For further information visit https://errors.pydantic.dev/2.13/v/missing
STDOUT: {"jsonrpc":"2.0","id":0,"error":{"code":-32602,"message":"Invalid request parameters","data":""}}
STDIN: {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}},"roots":{},"sampling":{},"elicitation":{}},"clientInfo":{"name":"goose-desktop","version":"0.0.0"}}}
STDOUT: {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"experimental":{},"prompts":{"listChanged":false},"resources":{"subscribe":false,"listChanged":false},"tools":{"listChanged":true},"tasks":{"list":{},"cancel":{},"requests":{"tools":{"call":{}},"prompts":{"get":{}},"resources":{"read":{}}}}},"serverInfo":{"name":"mymcp","version":"2.14.4"}}}
STDIN: {"jsonrpc":"2.0","method":"notifications/initialized"}
STDIN: {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"agent-session-id":"test-session-id","progressToken":0}}}
STDOUT: {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"divide","description":"Divide two numbers and return the result.","inputSchema":{"properties":{"dividend":{"description":"Dividend/numerator of the division.","type":"number"},"divisor":{"description":"Divisor/denominator of the division.","type":"number"}},"required":["dividend","divisor"],"type":"object"},"outputSchema":{"description":"Generic wrapper for non-object return types.","properties":{"result":{"type":"number"}},"required":["result"],"type":"object","x-fastmcp-wrap-result":true},"_meta":{"_fastmcp":{"tags":[]}}}]}}
STDIN: {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"_meta":{"agent-session-id":"test-session-id","agent-tool-call-request-id":"test-id","progressToken":1},"name":"divide","arguments":{"dividend":10,"divisor":2}}}
STDOUT: {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"5.0"}],"structuredContent":{"result":5.0},"isError":false}}
