# list all tasks
default:
    @just --list --unsorted

# run tests
test *FLAGS:
    uv run pytest tests -m "not integration" {{ FLAGS }}

# run integration tests
integration *FLAGS:
    uv run pytest tests -m integration {{ FLAGS }}

# format code
format:
    #!/usr/bin/env bash
    UVX_PATH="$(which uvx)"

    if [ -z "$UVX_PATH" ]; then
      echo "[error]: unable to find uvx"
      exit 1
    fi
    eval "$UVX_PATH ruff format ."
    eval "$UVX_PATH ruff check . --fix"

    just --unstable --fmt

# run tests with coverage
coverage *FLAGS:
    uv run coverage run -m pytest tests -m "not integration" {{ FLAGS }}
    uv run coverage report
    uv run coverage lcov -o lcov.info

# build docs
docs:
    uv sync && uv run mkdocs serve

# install pre-commit hooks
install-hooks:
    #!/usr/bin/env bash
    HOOKS_DIR="$(git rev-parse --git-path hooks)"

    if [ ! -d "$HOOKS_DIR" ]; then
      mkdir -p "$HOOKS_DIR"
    fi

    cat > "$HOOKS_DIR/pre-commit" <<EOF
    #!/usr/bin/env bash

    just format
    EOF

    if [ ! -f "$HOOKS_DIR/pre-commit" ]; then
      echo "[error]: failed to create pre-commit hook at $HOOKS_DIR/pre-commit"
      exit 1
    fi
    echo "installed pre-commit hook to $HOOKS_DIR"
    chmod +x "$HOOKS_DIR/pre-commit"

# get latest ai-exchange version from pypi
ai-exchange-version:
    #!/usr/bin/env bash
    curl --silent https://pypi.org/pypi/ai-exchange/json |
      jq --raw-output ".info.version"

# bump goose and ai-exchange version
release version:
    #!/usr/bin/env bash
    if [[ ! "{{ version }}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-.*)?$ ]]; then
      echo "[error]: invalid version '{{ version }}'."
      echo "  expected: semver format major.minor.patch or major.minor.patch-<suffix>"
      exit 1
    fi
    uvx --from=toml-cli toml set --toml-path=pyproject.toml "project.version" {{ version }}
    git checkout -b "release-{{ version }}"
    git add pyproject.toml
    git commit --message "chore(release): release version {{ version }}"

# extract tag from pyproject.toml
get-tag-version:
    #!/usr/bin/env bash
    uvx --from=toml-cli toml get --toml-path=pyproject.toml "project.version"

# create tag from pyproject.toml
tag:
    #!/usr/bin/env bash
    git tag v$(just get-tag-version)

# create tag and push to origin (use this when release branch is merged to main)
tag-push: tag
    #!/usr/bin/env bash
    # this will kick of ci for release
    git push origin tag v$(just get-tag-version)

# create release notes latest tag..HEAD
release-notes:
    #!/usr/bin/env bash
    git log --pretty=format:"- %s" v$(just get-tag-version)..HEAD

# setup langfuse server
langfuse-server:
    #!/usr/bin/env bash
    ./scripts/setup_langfuse.sh
