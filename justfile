default:
  @just --list --unsorted

test *FLAGS:
  uv run pytest tests -m "not integration" {{FLAGS}}

integration *FLAGS:
  uv run pytest tests -m integration {{FLAGS}}

format:
  #!/usr/bin/env bash
  UVX_PATH="$(which uvx)"

  if [ -z "$UVX_PATH" ]; then
    echo "[error]: unable to find uvx"
    exit 1
  fi
  eval "$UVX_PATH ruff format ."
  eval "$UVX_PATH ruff check . --fix"


coverage *FLAGS:
  uv run coverage run -m pytest tests -m "not integration" {{FLAGS}}
  uv run coverage report
  uv run coverage lcov -o lcov.info

docs:
  uv sync && uv run mkdocs serve

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

ai-exchange-version:
  curl -s https://pypi.org/pypi/ai-exchange/json | jq -r .info.version

# bump project version, push, create pr
release version:
  uvx --from=toml-cli toml set --toml-path=pyproject.toml project.version {{version}}
  ai_exchange_version=$(just ai-exchange-version) && sed -i '' 's/ai-exchange>=.*/ai-exchange>='"${ai_exchange_version}"'\",/' pyproject.toml
  git checkout -b release-version-{{version}}
  git add pyproject.toml
  git commit -m "chore(release): release version {{version}}"

tag_version:
  grep 'version' pyproject.toml | cut -d '"' -f 2

tag:
  git tag v$(just tag_version)

# this will kick of ci for release
# use this when release branch is merged to main
tag-push:
  just tag
  git push origin tag v$(just tag_version)

# get commit messages for a release
release-notes:
  git log --pretty=format:"- %s" v$(just tag_version)..HEAD
