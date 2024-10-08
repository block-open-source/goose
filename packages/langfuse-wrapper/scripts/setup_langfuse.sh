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
#   - Docker and Docker Compose
#   - curl
#   - A .env.langfuse.local file in the env directory
#
# Note: This script is intended for local development use only.

set -e

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
LANGFUSE_DOCKER_COMPOSE_URL="https://raw.githubusercontent.com/langfuse/langfuse/main/docker-compose.yml"
LANGFUSE_DOCKER_COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yaml"
LANGFUSE_ENV_FILE="$SCRIPT_DIR/../env/.env.langfuse.local"

download_docker_compose() {
    curl -o "$LANGFUSE_DOCKER_COMPOSE_FILE" "$LANGFUSE_DOCKER_COMPOSE_URL"
}

start_docker_compose() {
    docker compose --env-file "$LANGFUSE_ENV_FILE" -f "$LANGFUSE_DOCKER_COMPOSE_FILE" up --detach
}

wait_for_service() {
    echo "Waiting for Langfuse to start..."
    while ! nc -z localhost 3000; do   
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
        echo "Please log in with the following credentials:"
        grep -E "LANGFUSE_INIT_USER_EMAIL|LANGFUSE_INIT_USER_PASSWORD" "$LANGFUSE_ENV_FILE"
    else
        echo "Langfuse environment file with local credentials not found."
    fi
}

download_docker_compose
start_docker_compose
wait_for_service
print_login_variables
launch_browser