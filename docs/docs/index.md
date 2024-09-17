# block-goose

Github: [`squareup/goose-plugins-block`][squareup/goose-plugins-block]
Slack channel: [#goose-users](https://square.enterprise.slack.com/archives/C06PBQ52MCK)

This documentation describes Block's internal version of the open-source Goose AI tool. This version extends the open source `goose` with Block-specific LLM providers, toolkits, and  CLI commands. You can think of it as goose for Block.

We use goose-plugins-block to install goose along with those block extensions. Installing this package will install everything you need, including `goose` and its dependencies. See the [installation][install] instructions. 

### Currently available commands
> [!NOTE]
> Goose is currently in active development - we are adding new CLI commands on a daily basis. Please browse `goose --help` and `goose <subcommand> --help` to discover new additions to the CLI.

> [!IMPORTANT]
> Please note that all of these commands must be prefixed with `sq` if you installed Goose via the `sq` toolchain. For example, `sq goose session start`

```bash
# learn more about the cli!
goose --help
goose session --help

# starts a brand new goose session
# if you are new to goose, this is where you should start
goose session start

# starts a new goose session with the specified profile config
goose session start --profile <profile_name>

# lists out all the past goose sessions you have saved
goose session list

# resumes a past session via its name identifier
goose session resume <session_name>

# Starts a sessions and executes the prompt and tasks in the plan.yaml
# cat plan.yaml
# kickoff_message: |
#    Summarize this project
#
#tasks:
# - use ripgrep to get all the relevant files
# - summarize each file
goose session start --plan plan.yaml

# allows you to select past sessions to share via Square Console.
goose share

# prints out the version of goose and any plugins you have installed (e.g. goose-plugins-block)
goose version

# creates / updates unit tests associated with the indicated file
goose unit-test-gen <file>

# migrates python code in the current directory from Prefect 1 to Prefect 2
goose migrate-prefect
```

### goose
* PyPi package name: [`goose-ai`][goose-ai]
* GitHub: [square/goose]

### exchange
* PyPi package name: [`ai-exchange`][ai-exchange]
* GitHub: [square/exchange]

[goose-ai]: https://pypi.org/project/goose-ai/
[square/goose]: https://github.com/square/goose
[ai-exchange]: https://pypi.org/project/ai-exchange/
[square/exchange]: https://github.com/square/exchange
[squareup/goose-plugins-block]: https://github.com/squareup/goose-plugins-block
[install]: installation.md
