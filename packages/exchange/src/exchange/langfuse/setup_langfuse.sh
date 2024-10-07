#!/bin/bash

# setup_langfuse.sh
#
# This script sets up and runs Langfuse locally for development and testing purposes.
#
# Key functionalities:
# 1. Clones or updates the Langfuse repository
# 2. Copies the local environment file
# 3. Starts Langfuse using Docker Compose with default initialization variables.
# 4. Waits for the service to be available
# 5. Launches a browser to open the local Langfuse UI
# 6. Prints login credentials from the environment file
#
# Usage:
#   ./setup_langfuse.sh
#
# Requirements:
#   - Git
#   - Docker and Docker Compose
#   - A .env.langfuse.local file in the same directory as this script. You may modify .env.langfuse.local to update default initialization credentials.
#
# Note: This script is intended for local development use only.


set -e

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
LANGFUSE_REPO_URL="https://github.com/langfuse/langfuse.git"
LANGFUSE_CLONE_DIR="$SCRIPT_DIR/langfuse_clone"
LANGFUSE_LOCAL_ENV_FILE_NAME=".env.langfuse.local"
LANGFUSE_LOCAL_INIT_ENV_FILE="$SCRIPT_DIR/$LANGFUSE_LOCAL_ENV_FILE_NAME"

update_or_clone_repo() {
    if [ -d "$LANGFUSE_CLONE_DIR/.git" ]; then
        git -C "$LANGFUSE_CLONE_DIR" pull --rebase
    else
        git clone "$LANGFUSE_REPO_URL" "$LANGFUSE_CLONE_DIR"
    fi
}

copy_env_file() {
    if [ -f "$LANGFUSE_LOCAL_INIT_ENV_FILE" ]; then
        cp "$LANGFUSE_LOCAL_INIT_ENV_FILE" "$LANGFUSE_CLONE_DIR"
    else
        echo "Environment file not found. Exiting."
        exit 1
    fi
}

start_docker_compose() {
    cd "$LANGFUSE_CLONE_DIR" && docker compose --env-file "./$LANGFUSE_LOCAL_ENV_FILE_NAME" up --detach
}

wait_for_service() {
    while ! nc -z localhost 3000; do   
        sleep 1
    done
}

launch_browser() {
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        xdg-open "http://localhost:3000"
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        open "http://localhost:3000"
    else
        echo "Please open http://localhost:3000 to view Langfuse traces."
    fi
}

print_login_variables() {
    if [ -f "$LANGFUSE_LOCAL_INIT_ENV_FILE" ]; then
        echo "Please log in with the initialization environment variables:"
        grep -E "LANGFUSE_INIT_USER_EMAIL|LANGFUSE_INIT_USER_PASSWORD" "$LANGFUSE_LOCAL_INIT_ENV_FILE"
    else
        echo "Langfuse environment file with local credentials required for local login not found."
    fi
}

update_or_clone_repo
copy_env_file
start_docker_compose
wait_for_service
launch_browser
print_login_variables