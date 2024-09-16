
Your reply will be used to call a tool, so it **must** be of the form of a json markdown block that confirms
to the specified json schema. For example, for the following schema for "greet":

```json
{'type': 'object', 'properties': {'who': {'type': 'string', 'description': 'Who to greet'}, 'message': {'type': 'string', 'description': 'The content of the greeting'}}, 'required': ['who', 'message']}
```

a valid reply would be:

```json
{'who': 'world', 'message': 'hello'}
```

For this request, the tool's description is:

```
{{tool.description}}
```

The tool's schema is:

```json
{{tool.parameters}}
```
