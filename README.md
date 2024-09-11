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

**Welcome to `goose`, the Open-Source, Extensible AI Coding Assistant!** 

Unlike traditional coding tools, `goose` leverages advanced natural language processing to interpret complex commands into actionable, executable sequences. This allows for a coding assistant that not only writes code but also understands the intent behind your commands, enabling it to manage broader project scopes autonomously.

> [!TIP]
> **Quick install:**
> ```
> pipx install goose-ai
> ```




`TODO: slow video down and center it on the page`

![goose_demo](https://github.com/user-attachments/assets/0794eaba-97ab-40ef-af64-6fc7f68eb8e2)

## What Sets Goose Apart?

* **Adaptability**: `goose` excels in its ability to adapt to changes in code in real-time, making necessary adjustments as errors occur. This mimics a human developer's approach to coding, iteratively refining the work until perfection is achieved.
* **Autonomy**: While tools like Copilot provide line-by-line code suggestions based on comments or partial code snippets, goose can take full command descriptions to manage entire projects with little to no ongoing input from developers.
* **Customization and Extensibility**: Open-source and fully customizable, goose is built to support extensive user-defined modifications, unlike the mostly closed systems of Copilot and Cursor. Developers can tailor `goose` extensively to fit specific workflows, integrate with various toolkits, and enhance functionality through community contributions.
* **Privacy and Security**: Operating locally on your device, `goose` ensures that your proprietary code and sensitive data are never sent to the cloud.

## Features and capabilities
`#TODO: include a slimmed-down version of table but add some more descriptions on goose's capabilities (a few sentences)`

|  | goose | aider | copilot | cursor |
| --- | --- | --- | --- | --- |
| open source | ✅ | ✅ | ❌ | ❌ |
| in terminal | ✅ | ✅ | ❌ | ❌ |
| VSCode extension | ✅ | ✅ | ✅ | ❌ |
| Jetbrains extension  | WIP | ✅ | ✅ | ❌ |
| Anthropic | ✅ | ✅ | ❌ | ✅ |
| OpenAI | ✅ | ✅ | ✅ | ✅ |
| Ollama | ✅ | ✅ | ❌ | ❌ |
| Gemini | ❌ | ✅ | ❌ | ❌ |
| Bedrock | ✅ | ✅ | ❌ | ❌ |
| Azure | ✅ | ✅ | ❌ | ❌ |
| Cohere | ❌ | ✅ | ❌ | ❌ |
| DeepSeek | ❌ | ✅ | ❌ | ❌ |
| Groq | ❌ | ✅ | ❌ | ❌ |
| VertexAI | ❌ | ✅ | ❌ | ❌ |
| Databricks | ✅ | ❌ | ❌ | ❌ |
| Custom response instructions | ✅ | ✅ | ❔ | ✅ |
|  |  |  |  |  |

## What Block employees have to say about `goose`

`# TODO: pick the best ones`

`NOTE: may want to ask permission before adding names`

> Hi team, thank you for much for making Goose, it's so amazing. Our team is working on migrating Dashboard components to React  components. I am working on adopting Goose to help the migration.

-- K, Software Engineer

> I wanted to try to change the `sql-unit` tool to take a specific file path rather than a directory on the CLI and I'm amazed it did it... It was hands off after I told it what I wanted and how to run tests.

-- J, Software Engineer

> Got goose to update a dependency, run tests, make a branch and a commit... it was :chef-kiss:. Not that complicated but it figured out how to run tests from the readme which was nice.

-- J, Software Engineer

>  > With Goose, I feel like I am Maverick.
> 
> Thanks a ton for creating this. :thank:
> I have been having way too much fun with it today.

-- P, Machine Learning Engineer

> Hi Team, just wanted to share my experience of using goose as a non-engineer!! I had previously installed compost data several months back during a training and I wanted to make sure my environment was up to date. I just asked Goose to ensure that my environment is up to date and copied over this guide into my prompt. Goose managed everything flawlessly, keeping me informed at every step... I was truly [impresssed] how well it works and how easy it was to get started! :heart_eyes:

-- M, Product Manager

### Examples

`#TODO: rewrite/reformat`

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

`# TODO: add information for the new JetBrains plugin`

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

`TODO: expand on this section`

`goose` can be extended with toolkits, and out of the box there are some available: 

* `screen`: for letting goose take a look at your screen to help debug or work on designs (gives goose eyes)
* `github`: for awareness and suggestions on how to use github
* `repo_context`: for summarizing and understanding a repository you are working in.

### Examples

`remove this section?`
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

`Tips are removed and can be added to more extensive documentation elsewhere?`

## FAQ

`FAQs are removed and can be added to more extensive documentation elsewhere?`

## Open Source

Yes, `goose` is open source and always will be. `goose` is released under the ASL2.0 license meaning you can use it however you like. 
See LICENSE.md for more details.

To run `goose` from source, please see `CONTRIBUTING.md` for instructions on how to set up your environment and you can then run `uv run `goose` session start`.


[pipx]: https://github.com/pypa/pipx?tab=readme-ov-file#install-pipx
