# Contributing

<p>
<a href="#prerequisites">Prerequisites</a> •
<a href="#developing-and-testing">Developing and testing</a> •
<a href="#building-from-source">Building from source</a> •
<a href="#developing-goose-plugins">Developing goose-plugins</a> •
<a href="#running-ai-exchange-from-source">Running ai-exchange from source</a> •
<a href="#evaluations">Evaluations</a> •
<a href="#conventional-commits">Conventional Commits</a>
</p>

We welcome Pull Requests for general contributions. If you have a larger new feature or any questions on how to develop a fix, we recommend you open an [issue][issues] before starting.

## Prerequisites

We provide a shortcut to standard commands using [just][just] in our `justfile`.

Goose uses [uv][uv] for dependency management, and formats with [ruff][ruff] - install UV first: https://pypi.org/project/uv/ 

## Developing and testing

Now that you have a local environment, you can make edits and run our tests. 

### Creating plugins

Goose is highly configurable through plugins - it reads in modules that its dependencies install (e.g.`goose-plugins`) and uses those that start with certain prefixes (e.g. `goose.toolkit`) to inject their functionality. For example, you will note that Goose's CLI is actually merged with additional CLI methods that are exported from `goose-plugins`.

If you are building a net new feature, you should try to fit it inside a plugin. Goose and `goose-plugins` both support plugins, but there's an important difference in how contributions to each are reviewed. Use the guidelines below to decide where to contribute:

**When to Add to Goose**:

Plugins added directly to Goose are subject to rigorous review. This is because they are part of the core system and need to meet higher standards for stability, performance, and maintainability, often being validated through benchmarking.

**When to Add to `goose-plugins`:**

Plugins in `goose-plugins` undergo less detailed reviews and are more modular or experimental. They can prove their value through usage or iteration over time and may be eventually moved over to Goose.

To see how to add a toolkit, see the [toolkits documentation][adding-toolkit].

### Running tests
```sh
uv run pytest tests -m "not integration"
```

or, as a shortcut, 

```sh
just test
```

## Building from source

If you want to develop features on `goose`:

1. Clone Goose:
 ```bash
 git clone git@github.com:square/goose.git ~/Development/goose
 ```
2. Get `uv` with `brew install uv`
3. Set up your Python virtualenv:
```bash
cd ~/Development/goose
uv sync
uv venv
```
4. Run the `source` command that follows the `uv venv` command to activate the virtualenv.
5. Run Goose:
```bash
uv run goose session start  # or any of goose's commands (e.g. goose --help)
```

### Running from source

When you build from source you may want to run it from elsewhere.

1. Run `uv sync` as above
2. Run ```export goose_dev=`uv run which goose` ```
3. You can use that from anywhere in your system, for example `cd ~/ && $goose_dev session start`, or from your path if you like (advanced users only) to be running the latest.

## Developing goose-plugins

1. Clone the `goose-plugins` repo:
```bash
 git clone git@github.com:square/goose-plugins.git ~/Development/goose-plugins
```
2. Follow the steps for creating a virtualenv in the `goose` section above
3. Install `goose-plugins` in `goose`. This means any changes to `goose-plugins` in this folder will immediately be reflected in `goose`:
```bash
uv add --editable ~/Development/goose-plugins
```
4. Make your changes in `goose-plugins`, add the toolkit to the `profiles.yaml` file and run `uv run goose session --start` to see them in action.

## Running ai-exchange from source

goose depends heavily on the [`ai-exchange`][ai-exchange] project, you can clone that repo and then work on both by running: 

```sh
uv add --editable <path/to/cloned/exchange>
```

then when you run goose with `uv run goose session start` it will be running it all from source. 

## Evaluations

Given that so much of Goose involves interactions with LLMs, our unit tests only go so far to confirming things work as intended.

We're currently developing a suite of evaluations, to make it easier to make improvements to Goose more confidently.

In the meantime, we typically incubate any new additions that change the behavior of the Goose through **opt-in** plugins - `Toolkit`s, `Moderator`s, and `Provider`s. We welcome contributions of plugins that add new capabilities to *goose*. We recommend sending in several examples of the new capabilities in action with your pull request.

Additions to the [developer toolkit][developer] change the core performance, and so will need to be measured carefully.

## Conventional Commits

This project follows the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) specification for PR titles. Conventional Commits make it easier to understand the history of a project and facilitate automation around versioning and changelog generation.

[issues]: https://github.com/square/goose/issues
[goose-plugins]: https://github.com/square/goose-plugins
[ai-exchange]: https://github.com/square/exchange
[developer]: src/goose/toolkit/developer.py
[uv]: https://docs.astral.sh/uv/
[ruff]: https://docs.astral.sh/ruff/
[just]: https://github.com/casey/just
[adding-toolkit]: https://square.github.io/goose/configuration.html#adding-a-toolkit
