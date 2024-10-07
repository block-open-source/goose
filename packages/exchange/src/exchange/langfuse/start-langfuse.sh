#!/bin/bash

set -e  # Exit immediately if any command fails

SCRIPT_DIR=$(dirname "$(realpath "$0")")
REPO_URL="https://github.com/langfuse/langfuse.git"
CLONE_DIR="$SCRIPT_DIR/langfuse_clone"
ENV_FILE="$SCRIPT_DIR/local-langfuse.env"

# Function to update or clone the langfuse repository
update_or_clone_repo() {
    if [ -d "$CLONE_DIR/.git" ]; then
        echo "Repository exists. Pulling latest changes..."
        git -C "$CLONE_DIR" pull --rebase
    else
        echo "Cloning repository..."
        git clone "$REPO_URL" "$CLONE_DIR"
    fi
}

# Copy environment file with default initialization variables
copy_env_file() {
    if [ -f "$ENV_FILE" ]; then
        echo "Copying environment file..."
        cp "$ENV_FILE" "$CLONE_DIR/"
    else
        echo "Environment file not found. Exiting."
        exit 1
    fi
}

# Start the Docker containers for the Langfuse service and Postgres db
start_docker_compose() {
    echo "Starting Docker containers..."
    (cd "$CLONE_DIR" && docker compose --env-file ./local-langfuse.env up --detach)
}

# Wait for localhost:3000 and launch the browser
launch_browser() {
    echo "Waiting for service on localhost:3000 to be available..."
    while ! nc -z localhost 3000; do
        sleep 1
    done

    echo "Service is running on localhost:3000. Launching browser..."
    case "$OSTYPE" in
      linux*)   xdg-open http://localhost:3000 ;;  # Linux
      darwin*)  open http://localhost:3000 ;;      # macOS
      *)        echo "Please open http://localhost:3000 manually." ;;
    esac
}

# Read and print default email/password login from the environment file
print_env_variables() {
    if [ -f "$ENV_FILE" ]; then
        echo "Please login with the initialization environment variables:"
        source "$ENV_FILE"
        echo "EMAIL: $LANGFUSE_INIT_USER_EMAIL"
        echo "PASSWORD: $LANGFUSE_INIT_USER_PASSWORD"
    else
        echo "Environment file not found. Exiting."
        exit 1
    fi
}

# Main script execution
update_or_clone_repo
copy_env_file
start_docker_compose
launch_browser
print_env_variables