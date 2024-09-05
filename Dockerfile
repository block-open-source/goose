# Use an official Python runtime as a parent image
FROM python:3.10-slim

# Set the working directory in the container
WORKDIR /app

# Install SSL certificates, update certifi, install pipx and move to path
RUN apt-get update && apt-get install -y ca-certificates git curl make \
    && pip install --upgrade certifi \
    && pip install pipx \
    && pipx ensurepath

# Install goose-ai CLI using pipx
RUN pipx install goose-ai

# Make sure the PATH is updated
ENV PATH="/root/.local/bin:${PATH}"

# Run an infinite loop to keep the container running for testing
ENTRYPOINT ["goose", "session", "start"]

# once built, you can run this with something like: 
#    docker run -it --env OPENAI_API_KEY goose-ai
# or to run against ollama running on the same host
#    docker run -it --env OPENAI_HOST=http://host.docker.internal:11434/ --env OPENAI_API_KEY=unused goose-ai
#
#   To use goose in a docker style sandbox for experimenting.