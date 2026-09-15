---
title: Using goose in Headless Mode for Automation
description: goose Headless Mode
---

# Automate Development Tasks with goose Headless Mode

*Run AI-powered engineering workflows in CI/CD pipelines, servers, and batch processing scenarios*

The ability to automate complex engineering tasks without human intervention has been huge, but let's take it to the next level with AI. goose's "headless" mode enables developers to harness the full power of AI automation in server environments, CI/CD pipelines, and batch processing scenarios where interactive sessions simply aren't feasible.

## What is Headless Mode?

Headless mode is goose's non-interactive execution environment, designed for automated scenarios where human intervention isn't available (or wanted).

Unlike the interactive desktop app or CLI sessions, headless mode processes instructions and exits automatically, making it perfect for integration into existing development workflows.

Think of it as the difference between having a conversation with an AI assistant versus sending it on a side quest with clear instructions and trusting it to complete the task autonomously.

## Interactive vs. Headless: Understanding the Differences

| Feature | Interactive Mode | Headless Mode |
|---------|------------------|---------------|
| **User Input** | Prompts for user decisions and clarifications | Uses default behaviors and pre-configured settings |
| **Context Management** | Prompts user to choose strategy when limits reached | Automatically summarizes conversations |
| **Session Persistence** | Maintains ongoing conversation state | Executes task and exits cleanly |
| **Error Handling** | User can intervene and provide guidance | Automated error responses based on configuration |
| **Tool Permissions** | Can prompt for approval of risky operations | Uses configured defaults or fails safely |
| **Execution Flow** | Back-and-forth conversation style | Single execution with comprehensive output |

## Real-World Use Cases with Command Examples

### 1. Server Environments and Cloud Deployments

Perfect for headless servers, containerized environments, and cloud deployments where GUI access isn't available.

```bash
# Automated server maintenance
goose run --with-builtin developer -t "Check system logs for errors in the last 24 hours, identify performance bottlenecks, and generate a maintenance report"

# Container optimization
goose run --no-session -t "Analyze the Dockerfile, optimize for smaller image size, and update the build process documentation"

# Cloud resource audit
goose run -t "Review our AWS infrastructure configuration, identify cost optimization opportunities, and create a migration plan for underutilized resources"
```

### 2. CI/CD Pipeline Integration

Seamlessly integrate AI-powered analysis and fixes into your continuous integration workflows.

```bash
# In your .github/workflows/ci.yml
- name: AI-Powered Code Review
  run: |
    goose run --with-builtin developer \
      -t "Analyze the code changes in this PR, check for security vulnerabilities, performance issues, and suggest improvements. Generate a detailed review report."

# Test failure analysis
goose run --debug -t "Examine the failing test suite, identify the root cause of failures, implement fixes, and ensure all tests pass"

# Automated documentation updates
goose run -t "Review code changes and update the README.md and API documentation to reflect new features and modifications"
```

### 3. Batch Processing and Bulk Operations

Handle large-scale operations across multiple files, repositories, or systems.

```bash
# Bulk code modernization
goose run --with-builtin developer \
  -t "Upgrade all Python files in the src/ directory from Python 3.8 to 3.11 syntax, update dependencies, and ensure compatibility"

# Multi-repository maintenance
for repo in repo1 repo2 repo3; do
  cd $repo
  goose run --no-session -t "Update all dependencies to latest stable versions, run tests, and create a PR if changes are needed"
  cd ..
done

# Database migration automation
goose run -t "Analyze the current database schema, generate migration scripts for the new requirements, and create rollback procedures"
```

### 4. Scheduled Job Execution

Combine with cron jobs or task schedulers for regular automated maintenance.

```bash
# Daily security scan (add to crontab)
0 2 * * * /usr/local/bin/goose run --no-session -t "Run comprehensive security audit, check for vulnerabilities, and email report to security team"

# Weekly dependency updates
0 9 * * 1 /usr/local/bin/goose run -t "Check for outdated dependencies, create update PRs for non-breaking changes, and schedule review for major updates"
```

## Best Practices for Headless Success

So how do we set all this up?

Let's talk about the foundations of successful headless automation:

### 1. **Craft Crystal-Clear Instructions**

Your instructions are the blueprint for a successful tool. Your prompt idea needs to be specific, detailed, and unambiguous. If in doubt, try some [vibe prompting to get you started](https://www.youtube.com/watch?v=IjXmT0W4f2Q).

```bash
# Good: Specific and actionable
goose run -t "Analyze the test failures in the latest CI run, identify the root cause, and create a fix with appropriate unit tests"

# Better: Even more detailed
goose run -t "Review the failed tests in tests/integration/, identify why the authentication middleware is failing, implement a fix that maintains backward compatibility, and add regression tests"
```

### 2. **Pre-Configure Your Environment**
Set up your environment variables to avoid some of the repetitive runtime decisions.

```bash
export GOOSE_CONTEXT_STRATEGY=summarize
export GOOSE_MAX_TURNS=50
export GOOSE_MODE=auto
export GOOSE_DISABLE_SESSION_NAMING=true
```

The `CONTEXT_STRATEGY` and `MAX_TURNS` settings help manage conversation limits, while `GOOSE_MODE` set to `auto` allows for non-interactive execution. `GOOSE_DISABLE_SESSION_NAMING` avoids the extra background model call used to generate a session name and keeps the default "CLI Session" name.

### 3. **Implement Robust Error Handling**

Always check exit codes and handle failures gracefully in your automation scripts:

```bash
#!/bin/bash
if ! goose run --no-session -t "Run security audit and fix critical issues"; then
    echo "goose automation failed - manual intervention required"
    exit 1
fi
```

### 4. **Choose the Right Session Strategy**
Use `--no-session` for one-off tasks to avoid cluttering your session history, but maintain sessions for complex, multi-step workflows that might need debugging later (or when trying this for the first time).


## Recipe Execution in Headless Mode

[Recipes](/docs/guides/recipes/) are goose's powerful way to define reusable, parameterized workflows. In headless mode, recipes become even more valuable as they enable sophisticated automation scenarios.

### Recipe Requirements for Headless Mode

For a recipe to work in headless mode, it **must** include a `prompt` field. This prompt serves as the initial instruction that kicks off the automated execution:

```yaml
# automation-recipe.yaml
title: "Automated Code Quality Check"
name: "Automated Code Quality Check"
description: "Comprehensive code quality analysis and improvement"
author:
  name: "DevOps Team"
  email: "devops@company.com"

# Required for headless mode
prompt: "Perform a comprehensive code quality analysis including linting, security scanning, test coverage analysis, and generate an improvement plan"

instructions: |
  You are an expert code quality engineer. Your task is to:
  1. Run static analysis tools (eslint, pylint, etc.)
  2. Perform security vulnerability scanning
  3. Analyze test coverage and identify gaps
  4. Check for code duplication and complexity issues
  5. Generate a prioritized improvement plan
  6. Create actionable tickets for the development team

parameters:
  - key: target_directory
    input_type: string
    requirement: required
    description: "Directory to analyze"
    default: "./src"
  - key: output_format
    input_type: string
    requirement: required
    description: "Report format (markdown, json, html)"
    default: "markdown"

extensions:
  - type: builtin
    name: developer
    display_name: Developer
    timeout: 300
    bundled: true
```

### Executing Recipes in Headless Mode

```bash
# Basic recipe execution
goose run --recipe automation-recipe.yaml

# With custom parameters
goose run --recipe automation-recipe.yaml \
  --params target_directory=./backend \
  --params output_format=json

# Complex workflow with multiple recipes
goose run --recipe main-workflow.yaml \
  --sub-recipe security-audit.yaml \
  --sub-recipe performance-analysis.yaml \
  --params environment=production
```

## Understanding the Limitations

While headless mode is incredibly powerful, it's important to understand its constraints, so you can set appropriate expectations for your automation ideas.

### 1. No User Interaction Capability

**What this means**: goose cannot ask for clarification, approval, or additional input during execution. If it's unsure of what to do, the prompt result will usually show you a question like "How should I proceed?".

**Impact**: If instructions are ambiguous or if unexpected situations arise, goose will make its best judgment based on available context, which might not always align with your intentions.

**Mitigation**: Provide extremely detailed instructions, especially on what to do if it runs into a problem, and **test your automation thoroughly in non-production** environments first.

```bash
# Problematic: Too vague
goose run -t "Fix the issues"

# Better: Specific and actionable
goose run -t "Fix the TypeScript compilation errors in src/components/, ensure all imports are correct, and update any deprecated API calls to use the latest syntax"
```

### 2. Recipe Prompt Requirements

**What this means**: Any recipe used in headless mode must include a `prompt` field, or execution will fail with an error.

**Impact**: Existing interactive recipes may need modification before they can be used in automated scenarios.

**Mitigation**: Always include meaningful prompts in your recipes, even if they're primarily designed for interactive use.

### 3. Tool Permission Dependencies

**What this means**: goose cannot prompt for permission to use potentially risky tools or operations.

**Impact**: Operations requiring approval will either use default permissions or fail, potentially blocking automation workflows.

**Mitigation**: Pre-configure tool permissions using environment variables:

```bash
export GOOSE_MODE=auto  # Automatically approve safe operations
# or configure specific tool permissions in your config
```

### 4. Context Decision Automation

**What this means**: When conversation context limits are reached, goose automatically applies the configured strategy without user input.

**Impact**: Important context might be lost if summarization isn't perfect, or execution might be interrupted if context clearing is too aggressive.

**Mitigation**: Configure appropriate context strategies and monitor token usage:

```bash
export GOOSE_CONTEXT_STRATEGY=summarize  # Usually the best choice for automation
export GOOSE_MAX_TURNS=100  # Prevent runaway execution
```

### 5. Error Recovery Limitations

**What this means**: Complex error scenarios that would benefit from human insight cannot be resolved interactively.

**Impact**: Automation might fail in edge cases that a human could easily resolve through conversation.

**Mitigation**: Implement comprehensive error handling in your calling scripts and have fallback procedures:

```bash
#!/bin/bash
if ! goose run --recipe complex-deployment.yaml; then
    # Fallback to simpler approach or alert human operators
    echo "Complex deployment failed, initiating rollback procedure"
    goose run --recipe rollback.yaml
fi
```

## Configuration and Environment Setup

### Essential Environment Variables

```bash
# Context management
export GOOSE_CONTEXT_STRATEGY=summarize
export GOOSE_MAX_TURNS=50

# Tool behavior
export GOOSE_MODE=auto

# Model configuration
export GOOSE_PROVIDER=openai
export GOOSE_MODEL=gpt-4o

# Output control
export GOOSE_CLI_MIN_PRIORITY=0.2  # Reduce verbose output
```

### Advanced Configuration

```bash
# Security and permissions
export GOOSE_ALLOWLIST=https://company.com/allowed-extensions.json
```

## The Future of Automated Development

goose's headless mode represents more than just a feature -- it's a shift toward truly automated development workflows powered by AI. We can remove human intervention in routine tasks so teams can focus on high-value work while AI handles the repetitive, time-consuming operations that slow us down.

Whether you're looking to streamline your CI/CD pipelines, automate server maintenance, or handle bulk operations across multiple repositories, goose's headless mode provides the foundation for building sophisticated, reliable automation workflows.

**Start your automation journey today:**

1. **Install goose** and configure your environment variables
2. **Create your first recipe** with clear prompts and detailed instructions  
3. **Test in a safe environment** before deploying to production
4. **Integrate with your existing workflows** and watch your productivity soar

Connect with us on our [Discord community](https://discord.gg/n8R5VaWDAn) to share your headless mode success stories, ask questions, and collaborate with other developers who want to push the boundaries of AI automation.
