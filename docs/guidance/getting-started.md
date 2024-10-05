# Your first run with Goose

This page contains two sections that will help you get started with Goose:

1. [Configuring Goose with the `profiles.yaml` file](#configuring-goose-with-the-profilesyaml-file): how to set up Goose with the right LLMs and toolkits.
2. [Working with Goose](#working-with-goose): how to guide Goose through a task, and how to provide context for Goose to work with.

## Configuring Goose with the `profiles.yaml` file
On the first run, Goose will detect what LLMs are available from your environment, and generate a configuration file at `~/.config/goose/profiles.yaml`. You can edit those profiles to further configure goose.

Here’s what the default `profiles.yaml` could look like if Goose detects an OpenAI API key:

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

You can edit this configuration file to use different LLMs and toolkits in Goose. Check out the [configuration docs][configuration] to better understand the different fields of the `profiles.yaml` file! You can add new profiles with different settings to change how goose works from one section to the next - use `goose session start --profile {profile}` to select which to use.

### LLM provider access setup

Goose works on top of LLMs.  You'll need to configure one before using it. By default, Goose uses `openai` as the LLM provider but you can customize it as needed. You need to set OPENAI_API_KEY as an environment variable if you would like to use `openai`.

To learn more about providers and modes of access, check out the [provider docs][providers].
```sh
export OPENAI_API_KEY=your_open_api_key
```

## Working with Goose

Goose works best with some amount of context or instructions for a given task. You can guide goose through gathering the context it needs by giving it instructions or asking it to explore with its tools. But to make this easier, context in Goose can be extended a few additional ways:

1. User-directed input
2. A `.goosehints` file
3. Toolkits
4. Plans

### User-directed input

Directing Goose to read a specific file before requesting changes ensures that the file's contents are loaded into its operational context. Similarly, asking Goose to summarize the current project before initiating changes across multiple files provides a detailed overview of the project structure, including the locations of specific classes, functions, and other components.

### `.goosehints`

If you are using the `developer` toolkit, `goose` adds the content from `.goosehints` file in the working directory to the system prompt. The hints file is meant to provide additional context about your project. The context can be user-specific or at the project level in which case, you can commit it to git. `.goosehints` file is Jinja templated so you could have something like this:

```
Here is an overview of how to contribute:
&#123;% include 'CONTRIBUTING.md' %&#125;

The following justfile shows our common commands:
&#123;% include 'justfile' %&#125;

Write all code comments in French
```

### Toolkits

Toolkits expand Goose’s capabilities and tailor its functionality to specific development tasks. Toolkits provide Goose with additional contextual information and interactive abilities, allowing for a more comprehensive and efficient workflow.

Here are some out-of-the-box examples:

* `developer`: for general-purpose development capabilities, including plan management, shell execution, and file operations, with default shell strategies like using ripgrep.
* `screen`: for letting goose take a look at your screen to help debug or work on designs (gives goose eyes)
* `github`: for suggestions on how to use Github
* `repo_context`: for summarizing and understanding a repository you are working in.
* `jira`: for working with JIRA (issues, backlogs, tasks, bugs etc.)

You can see the current toolkits available to Goose [here][available-toolkits]. There's also a [public plugins repository where toolkits are defined][goose-plugins] for Goose that has toolkits you can try out.

### Plans

Goose creates plans for itself to execute to achieve its goals. In some cases, you may already have a plan in mind for Goose — this is where you can define your own `plan.md` file, and it will set the first message and also hard code Goose's initial plan.

The plan.md file can be text in any format and uses `jinja` templating, and the last group of lines that start with “-” will be considered the plan.

Here are some examples:

#### Basic example plan

```md
Your goal is to refactor this fastapi application to use a sqlite database. Use `pytest -s -v -x` to run the tests when needed.

- Use ripgrep to find the fastapi app and its tests in this directory
- read the files you found
- Add sqlalchemy and alembic as dependencies with poetry
- Run alembic init to set up the basic configuration
- Add sqlite dependency with Poetry
- Create new module for database code and include sqlalchemy and alembic setup
- Define an accounts table with SQLAlchemy
- Implement CRUD operations for accounts table
- Update main.py to integrate with SQLite database and use CRUD operation
- Use alembic to create the table
- Use conftest to set up a test database with a new DB URL
- Run existing test suite and ensure all tests pass. Do not edit the test case behavior, instead use tests to find issues.
```

The starting plan is specified with the tasks. Each list entry is a different step in the plan. This is a pretty detailed set of tasks, but is really just a break-down of the conversation we had in the previous section.

The kickoff message is what gets set as the first user message when goose starts running (with the plan). This message should contain the overall goal of the tasks and could also contain extra context you want to include for this problem. In our case, we are just mentioning the test command we want to use to run the tests.

To run Goose with this plan:

``` sh
goose session start --plan plan.md
```

#### Injecting arguments into a plan

You can also inject arguments into your plan. `plan.md` files can be templated with `jinja` and can include variables that are passed in when you start the session.

The kickoff message gives Goose directions to use poetry and a dependency, and then a plan is to open a file, run a test, and set up a repo:

```md
Here is the python repo

- use {{ dep }}
- use poetry

Here is the plan:

- Open a file
- Run a test
- Set up {{ repo }}
```

To run Goose with this plan with the arguments `dep=pytest,repo=github`, you would run the following command:

```sh
goose session start --plan plan.md --args dep=pytest,repo=github
```

[configuration]: ../configuration.md
[available-toolkits]: ../plugins/available-toolkits.md
[providers]: ../plugins/providers.md
[goose-plugins]:https://github.com/block-open-source/goose-plugins
