# Contributing

We welcome Pull Requests for general contributions. If you have a larger new feature or any questions on how to develop a fix, we recommend you open an [issue][issues] before starting.

## Prerequisites

Goose uses [uv][uv] for dependency management, and formats with [ruff][ruff].
Clone goose and make sure you have installed `uv` to get started. When you use
`uv` below in your local goose directly, it will automatically setup the virtualenv
and install dependencies.

We provide a shortcut to standard commands using [just][just] in our `justfile`.

## Development

Now that you have a local environment, you can make edits and run our tests!

### Run Goose

If you've made edits and want to try them out, use

```
uv run goose session start
```

or other `goose` commands.

If you want to run your local changes but in another directory, you can use the path in
the virtualenv created by uv:

```
alias goosedev=`uv run which goose`
```

You can then run `goosedev` from another dir and it will use your current changes.

### Run Tests

To run the test suite against your edges, use `pytest`:

```sh
uv run pytest tests -m "not integration"
```

or, as a shortcut,

```sh
just test
```

## Exchange

The lower level generation behind goose is powered by the [`exchange`][ai-exchange] package, also in this repo.

Thanks to `uv` workspaces, any changes you make to `exchange` will be reflected in using your local goose. To run tests
for exchange, head to `packages/exchange` and run tests just like above

```sh
uv run pytest tests -m "not integration"
```

## Evaluations

Given that so much of Goose involves interactions with LLMs, our unit tests only go so far to confirming things work as intended.

We're currently developing a suite of evaluations, to make it easier to make improvements to Goose more confidently.

In the meantime, we typically incubate any new additions that change the behavior of the Goose through **opt-in** plugins - `Toolkit`s, `Moderator`s, and `Provider`s. We welcome contributions of plugins that add new capabilities to *goose*. We recommend sending in several examples of the new capabilities in action with your pull request.

Additions to the [developer toolkit][developer] change the core performance, and so will need to be measured carefully.

## Conventional Commits

This project follows the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) specification for PR titles. Conventional Commits make it easier to understand the history of a project and facilitate automation around versioning and changelog generation.

## Release

In order to release a new version of goose, you need to do the following:
1. Update CHANGELOG.md. To get the commit messages since last release, run: `just release-notes`
2. Update version in `pyproject.toml` for `goose` and package dependencies such as `exchange`
3. Create a PR and merge it into main branch
4. Tag the HEAD commit in main branch. To do this, switch to main branch and run: `just tag-push`
5. Publish a new release from the [Github Release UI](https://github.com/block-open-source/goose/releases)


[issues]: https://github.com/block-open-source/goose/issues
[goose-plugins]: https://github.com/block-open-source/goose-plugins
[ai-exchange]: https://github.com/block-open-source/goose/tree/main/packages/exchange
[developer]: https://github.com/block-open-source/goose/blob/dfecf829a83021b697bf2ecc1dbdd57d31727ddd/src/goose/toolkit/developer.py
[uv]: https://docs.astral.sh/uv/
[ruff]: https://docs.astral.sh/ruff/
[just]: https://github.com/casey/just
[adding-toolkit]: https://block-open-source.github.io/goose/configuration.html#adding-a-toolkit
