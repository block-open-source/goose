# Goose in action

This page is frequently updated with the latest use-cases and applications of Goose!

## Goose as a Github Action

**What it does**: 

An early version of a GitHub action that uses Goose to automatically address issues in your repository. It operates in the background to attempt fixes or enhancements based on issue descriptions.

The action attempts to fix issues described in GitHub. It takes the issue's title and body as input and tries to resolve the issue programmatically.

If the action successfully fixes the issue, it will automatically create a pull request with the fix. If it cannot confidently fix the issue, no pull request is created.

**Where you can find it**: https://github.com/marketplace/actions/goose-ai-developer-agent

**How you can do something similar**:

1. Decide what specific task you want Goose to automate. This could be anything from auto-linting code, updating dependencies, auto-merging approved pull requests, or even automating responses to issue comments.
2. In the `action.yml`, specify any inputs your action needs (like GitHub tokens, configuration files, specific command inputs) and outputs it may produce.
3. Write the script (e.g., Python or JavaScript) that Goose will use to perform the tasks. This involves setting up the Goose environment, handling GitHub API requests, and processing the task-specific logic.
