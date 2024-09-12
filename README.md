<h1 align="center">
goose
</h1>

<p align="center"><strong>goose</strong> <em>is a programming agent that runs on your machine.</em></p>

<p align="center">
<a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg"></a>
</p>

<p align="center">
<a href="#usage">Usage</a> •
<a href="#configuration">Configuration</a> •
<a href="#tips">Tips</a> •
<a href="#faq">FAQ</a> •
<a href="#open-source">Open Source</a>
</p>

`goose` assists in solving a wide range of programming and operational tasks. It is a live virtual developer you can interact with, guide, and learn from.

To solve problems, `goose` breaks down instructions into sequences of tasks and carries them out using tools. Its ability to connect its changes with real outcomes (e.g. errors) and course correct is its most powerful and exciting feature. `goose` is free open source software and is built to be extensible and customizable.

![goose_demo](https://github.com/user-attachments/assets/0794eaba-97ab-40ef-af64-6fc7f68eb8e2)



## Usage
### Installation

To install `goose`, we recommend `pipx`

First make sure you've [installed pipx][pipx] - for example

``` sh
brew install pipx
pipx ensurepath
```

Then you can install `goose` with

```sh
pipx install goose-ai
```
#### IDEs
There is an early version of a VS Code extension with goose support you can try here: https://github.com/square/goose-vscode - more to come soon.

### LLM provider access setup
`goose` works on top of LLMs (you need to bring your own LLM). By default, `goose` uses `openai` as LLM provider. You need to set OPENAI_API_KEY as an environment variable if you would like to use `openai`.
```sh
export OPENAI_API_KEY=your_open_api_key
```

Otherwise, please refer <a href="#configuration">Configuration</a> to customise `goose`

### Start `goose` session
From your terminal, navigate to the directory you'd like to start from and run:
```sh
goose session start
```

You will see a prompt `G❯`:

```
G❯ type your instructions here exactly as you would tell a developer.
```
Now you are interact with `goose` in conversational sessions - something like a natural language driven code interpreter.
The default toolkit lets it take actions through shell commands and file edits.
You can interrupt `goose` at any time to help redirect its efforts.

### Exit `goose` session
If you are looking to exit, use `CTRL+D`, although `goose` should help you figure that out if you forget. See below for some examples.


### Resume `goose` session
When you exit a session, it will save the history in `~/.config/goose/sessions` directory and you can resume it later on:

``` sh
goose session resume
```

## Configuration

`goose` can detect what LLM and toolkits it can work with from the configuration file `~/.config/goose/profiles.yaml` automatically.

### Configuration options
Example:

```yaml
default:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: truncate
  toolkits:
  - name: developer
    requires: {}
  - name: screen
    requires: {}
```

You can edit this configuration file to use different LLMs and toolkits in `goose`. `goose can also be extended to support any LLM or combination of LLMs

#### provider
Provider of LLM. LLM providers that currently are supported by `goose`:

| Provider        | Required environment variable(s) to access provider |
| :-----          | :------------------------------                     |
| openai          | `OPENAI_API_KEY`                                    |
| anthropic       | `ANTHROPIC_API_KEY`                                 |
| databricks      | `DATABRICKS_HOST` and `DATABRICKS_TOKEN`            |


#### processor
Model for complex, multi-step tasks such as writing code and executing commands. Example: `gpt-4o`.  You should choose the model based the provider you configured.

#### accelerator
Small model for fast, lightweight tasks. Example: `gpt-4o-mini`. You should choose the model based the provider you configured.

#### moderator
Rules designed to control or manage the output of the model. Moderators that currently are supported by `goose`:

- `passive`: does not actively intervene in every response
- `truncate`: truncates the first contexts when the contexts exceed the max token size

#### toolkits

`goose` can be extended with toolkits, and out of the box there are some available:

* `developer`: for general-purpose development capabilities, including plan management, shell execution, and file operations, with default shell strategies like using ripgrep.
* `screen`: for letting goose take a look at your screen to help debug or work on designs (gives goose eyes)
* `github`: for awareness and suggestions on how to use github
* `repo_context`: for summarizing and understanding a repository you are working in.

#### Configuring goose per repo

If you are using the `developer` toolkit, `goose` adds the content from `.goosehints`
 file in working directory to the system prompt of the `developer` toolkit. The hints
file is meant to provide additional context about your project. The context can be
user-specific or at the project level in which case, you
can commit it to git. `.goosehints` file is Jinja templated so you could have something
like this:
```
Here is an overview of how to contribute:
{% include 'CONTRIBUTING.md' %}

The following justfile shows our common commands:
```just
{% include 'justfile' %}
```

### Examples
#### provider as `anthropic`

```yaml
default:
  provider: anthropic
  processor: claude-3-5-sonnet-20240620
  accelerator: claude-3-5-sonnet-20240620
...
```
#### provider as `databricks`
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

## Tips

Here are some collected tips we have for working efficiently with `goose`

- **`goose` can and will edit files**. Use a git strategy to avoid losing anything - such as staging your
  personal edits and leaving `goose` edits unstaged until reviewed. Or consider using individual commits which can be reverted.
- **`goose` can and will run commands**. You can ask it to check with you first if you are concerned. It will check commands for safety as well.
- You can interrupt `goose` with `CTRL+C` to correct it or give it more info.
- `goose` works best when solving concrete problems - experiment with how far you need to break that problem
  down to get `goose` to solve it. Be specific! E.g. it will likely fail to `"create a banking app"`,
  but probably does a good job if prompted with `"create a Fastapi app with an endpoint for deposit and withdrawal
  and with account balances stored in mysql keyed by id"`
- If `goose` doesn't have enough context to start with, it might go down the wrong direction. Tell it
  to read files that you are referring to or search for objects in code. Even better, ask it to summarize
  them for you, which will help it set up its own next steps.
- Refer to any objects in files with something that is easy to search for, such as `"the MyExample class"
- `goose` *loves* to know how to run tests to get a feedback loop going, just like you do. If you tell it how you test things locally and quickly, it can make use of that when working on your project
- You can use `goose` for tasks that would require scripting at times, even looking at your screen and correcting designs/helping you fix bugs, try asking it to help you in a way you would ask a person.
- `goose` will make mistakes, and go in the wrong direction from times, feel free to correct it, or start again.
- You can tell `goose` to run things for you continuously (and it will iterate, try, retry) but you can also tell it to check with you before doing things (and then later on tell it to go off on its own and do its best to solve).
- `goose` can run anywhere, doesn't have to be in a repo, just ask it!


### Examples

Here are some examples that have been used:

```
G❯ Looking at the in progress changes in this repo, help me finish off the feature. CONTRIBUTING.md shows how to run the tests.
```

```
G❯ In this golang project, I want you to add open telemetry to help me get started with it. Look in the moneymovements module, run the `just test` command to check things work.
```

```
G❯ This project uses an old version of jooq. Upgrade to the latest version, and ensure there are no incompatibilities by running all tests. Dependency versions are in gradle/libs.versions.toml and to run gradle, use the binary located in bin/gradle
```

```
G❯ This is a fresh checkout of a golang project. I do not have my golang environment set up. Set it up and run tests for this project, and ensure they pass. Use the zookeeper jar included in this repository rather than installing zookeeper via brew.
```

```
G❯ In this repo, I want you to look at how to add a new provider for azure.
Some hints are in this github issue: https://github.com/square/exchange/issues
/4 (you can use gh cli to access it).
```

```
G❯ I want you to help me increase the test coverage in src/java... use mvn test to run the unit tests to check it works.
```

## FAQ

**Q:** Why did I get error message of "The model `gpt-4o` does not exist or you do not have access to it.` when I talked goose?

**A:** You can find out the LLM provider and models in the configuration file `~/.config/goose/profiles.yaml` here to check whether your LLM provider account has access to the models.  For example, after you have made a successful payment of $5 or more (usage tier 1), you'll be able to access the GPT-4, GPT-4 Turbo, GPT-4o models via the OpenAI API. [How can I access GPT-4, GPT-4 Turbo, GPT-4o, and GPT-4o mini?](https://help.openai.com/en/articles/7102672-how-can-i-access-gpt-4-gpt-4-turbo-gpt-4o-and-gpt-4o-mini).

## Open Source

Yes, `goose` is open source and always will be. `goose` is released under the ASL2.0 license meaning you can use it however you like.
See LICENSE.md for more details.

To run `goose` from source, please see `CONTRIBUTING.md` for instructions on how to set up your environment and you can then run `uv run `goose` session start`.


[pipx]: https://github.com/pypa/pipx?tab=readme-ov-file#install-pipx
