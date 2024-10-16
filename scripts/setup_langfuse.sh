#!/bin/bash

# setup_langfuse.sh
#
# This script sets up and runs Langfuse locally for development and testing purposes.
#
# Key functionalities:
# 1. Downloads the latest docker-compose.yaml from the Langfuse repository
# 2. Starts Langfuse using Docker Compose with default initialization variables
# 3. Waits for the service to be available
# 4. Launches a browser to open the local Langfuse UI
# 5. Prints login credentials from the environment file
#
# Usage:
#   ./setup_langfuse.sh
#
# Requirements:
#   - Docker
#   - curl
#   - A .env.langfuse.local file in the env directory
#
# Note: This script is intended for local development use only.

set -e

SCRIPT_DIR=$(realpath "$(dirname "${BASH_SOURCE[0]}")")
LANGFUSE_DOCKER_COMPOSE_URL="https://raw.githubusercontent.com/langfuse/langfuse/main/docker-compose.yml"
LANGFUSE_DOCKER_COMPOSE_FILE="langfuse-docker-compose.yaml"
LANGFUSE_ENV_FILE="$SCRIPT_DIR/../packages/exchange/.env.langfuse.local"

check_dependencies() {
    local dependencies=("curl" "docker")
    local missing_dependencies=()

    for cmd in "${dependencies[@]}"; do
        if ! command -v "$cmd" &> /dev/null; then
            missing_dependencies+=("$cmd")
        fi
    done

    if [ ${#missing_dependencies[@]} -ne 0 ]; then
        echo "Missing dependencies: ${missing_dependencies[*]}"
        exit 1
    fi
}

download_docker_compose() {
    if ! curl --fail --location --output "$SCRIPT_DIR/langfuse-docker-compose.yaml" "$LANGFUSE_DOCKER_COMPOSE_URL"; then
        echo "Failed to download docker-compose file from $LANGFUSE_DOCKER_COMPOSE_URL"
        exit 1
    fi
}

start_docker_compose() {
    docker compose --env-file "$LANGFUSE_ENV_FILE" -f "$LANGFUSE_DOCKER_COMPOSE_FILE" up --detach
}

wait_for_service() {
    echo "Waiting for Langfuse to start..."
    local retries=10
    local count=0
    until curl --silent http://localhost:3000 > /dev/null; do
        ((count++))
        if [ "$count" -ge "$retries" ]; then
            echo "Max retries reached. Langfuse did not start in time."
            exit 1
        fi
        sleep 1
    done
    echo "Langfuse is now available!"
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
    if [ -f "$LANGFUSE_ENV_FILE" ]; then
        echo "If not already logged in use the following credentials to log in:"
        grep -E "LANGFUSE_INIT_USER_EMAIL|LANGFUSE_INIT_USER_PASSWORD" "$LANGFUSE_ENV_FILE"
    else
        echo "Langfuse environment file with local credentials not found."
    fi
}

check_dependencies
pushd "$SCRIPT_DIR" > /dev/null
download_docker_compose
start_docker_compose
wait_for_service
print_login_variables
launch_browser
popd > /dev/null
