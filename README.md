<h1 align="center">
goose
</h1>

<p align="center"><strong>goose</strong> <em>is a programming agent that runs on your machine.</em></p>

<p align="center">
<a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg"></a>
</p>

<p align="center">
<a href="#usage">Usage</a> • 
<a href="#installation">Installation</a> •
<a href="#tips">Tips</a> 
</p>

`goose` assists in solving a wide range of programming and operational tasks. It is a live virtual developer you can interact with, guide, and learn from.

To solve problems, goose breaks down instructions into sequences of tasks and carries them out using tools. Its ability to connect its changes with real outcomes (e.g. errors) and course correct is its most powerful and exciting feature. goose is free open source software and is built to be extensible and customizable.

## Usage

You interact with goose in conversational sessions - something like a natural language driven code interpreter.
The default toolkit lets it take actions through shell commands and file edits.
You can interrupt Goose at any time to help redirect its efforts.

From your terminal, navigate to the directory you'd like to start from and run:
```sh
goose session start
```

You will see a prompt `G❯`:

```
G❯ type your instructions here exactly as you would tell a developer.
```

From here you can talk directly with goose - send along your instructions. If you are looking to exit, use `CTRL+D`,
although goose should help you figure that out if you forget.

When you exit a session, it will save the history and you can resume it later on:

``` sh
goose session resume
```

## Tips

Here are some collected tips we have for working efficiently with Goose

- **goose can and will edit files**. Use a git strategy to avoid losing anything - such as staging your
  personal edits and leaving goose edits unstaged until reviewed. Or consider using indivdual commits which can be reverted.
- You can interrupt goose with `CTRL+C` to correct it or give it more info.
- goose works best when solving concrete problems - experiment with how far you need to break that problem
  down to get goose to solve it. Be specific! E.g. it will likely fail to `"create a banking app"`, 
  but probably does a good job if prompted with `"create a Fastapi app with an endpoint for deposit and withdrawal
  and with account balances stored in mysql keyed by id"`
- If goose doesn't have enough context to start with, it might go down the wrong direction. Tell it
  to read files that you are refering to or search for objects in code. Even better, ask it to summarize
  them for you, which will help it set up its own next steps.
- Refer to any objects in files with something that is easy to search for, such as `"the MyExample class"
- goose *loves* to know how to run tests to get a feedback loop going, just like you do. If you tell it how you test things locally and quickly, it can make use of that when working on your project
- You can use goose for tasks that would require scripting at times, even looking at your screen and correcting designs/helping you fix bugs, try asking it to help you in a way you would ask a person. 
- goose will make mistakes, and go in the wrong direction from times, feel free to correct it, or start again.
- You can tell goose to run things for you continuously (and it will iterate, try, retry) but you can also tell it to check with you before doing things (and then later on tell it to go off on its own and do its best to solve).
- Goose can run anywhere, doesn't have to be in a repo, just ask it!

## Installation 

To install goose, we recommend `pipx`

First make sure you've [installed pipx][pipx] - for example

``` sh
brew install pipx
pipx ensurepath
```

Then you can install goose with 

``` sh
pipx install goose
```

### Config

Goose will try to detect what LLM it can work with and place a config in `~/.config/goose/profiles.yaml` automatically. 

#### Toolkits

Goose can be extended with toolkits, and out of the box there are some available: 

* `screen`: for letting goose take a look at your screen to help debug or work on designs (gives goose eyes)
* `github`: for awareness and suggestions on how to use github
* `repo_context`: for summarizing and understanding a repository you are working in.

To configure for example the screen toolkit, edit `~/.config/goose/profiles.yaml`:

```yaml
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
  - name: developer
    requires: {}
  - name: screen
    requires: {}
```
    


#### Advanced LLM config

goose works on top of LLMs (you bring your own LLM). If you need to customize goose, one way is via editing: `~/.config/goose/profiles.yaml`. 

It will look by default something like: 

```yaml
default:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: truncate
  toolkits:
  - name: developer
    requires: {}
```

*Note: This requires the environment variable `OPENAI_API_KEY` to be set to your OpenAI API key. goose uses at least 2 LLMs: one for acceleration for fast operating, and processing for writing code and executing commands.*

You can tell it to use another provider for example for Anthropic: 

```yaml
default:
  provider: anthropic
  processor: claude-3-5-sonnet-20240620
  accelerator: claude-3-5-sonnet-20240620
...
```

*Note: This will then use the claude-sonnet model, you will need to set the `ANTHROPIC_API_KEY` environment variable to your anthropic API key.* 

For Databricks hosted models: 

```yaml
default:
  provider: databricks
  processor: databricks-meta-llama-3-1-70b-instruct
  accelerator: databricks-meta-llama-3-1-70b-instruct
  moderator: passive
  toolkits:
  - name: developer
    requires: {}
```

This requires `DATABRICKS_HOST` and `DATABRICKS_TOKEN` to be set accordingly

(goose can be extended to support any LLM or combination of LLMs).

## Open Source

Yes, goose is open source and always will be. goose is released under the ASL2.0 license meaning you can use it however you like. 
See LICENSE.md for more details.


[pipx]: https://github.com/pypa/pipx?tab=readme-ov-file#install-pipx
