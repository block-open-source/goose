# Plugins in Goose

Goose's functionality is extended via plugins. These plugins fall into three main categories:

1. **Toolkits**: 
    * Provides Goose with tools (functions) it can call and optionally will load additional context into the system prompt (such as 'The Github CLI is called via `gh` and you should use it to run git commands'). 
    * Toolkits can do basically anything, from calling external APIs, to taking a screenshot of your screen, to summarizing your current project.
2. **CLI commands**: 
    * Provides additional commands to the Goose CLI. 
    * These commands can be used to interact with the Goose system, such as listing available toolkits or summarizing a session.
3. **Providers**: 
    * Provides Goose with access to external LLMs. 
    * For example, the OpenAI provider allows Goose to interact with the OpenAI API. 
    * Most providers for Goose are defined in the Exchange library.

