# Langfuse Wrapper

This package provides a wrapper for [Langfuse](https://langfuse.com/). The wrapper serves to initialize Langfuse appropriately if the Langfuse server is running locally and otherwise to skip applying the Langfuse observe descorators.

**Note: This Langfuse integration is experimental and we don't currently have integration tests for it.**


## Usage

### Start your local Langfuse server

Run `setup_langfuse.sh` to start your local Langfuse server. It requires Docker.

Read more about local Langfuse deployments [here](https://langfuse.com/docs/deployment/local).

### Exchange and Goose integration

Import `from langfuse_wrapper.langfuse_wrapper import observe_wrapper` and use the `observe_wrapper()` decorator on functions you wish to enable tracing for. `observe_wrapper` functions the same way as Langfuse's observe decorator.

Read more about Langfuse's decorator-based tracing [here](https://langfuse.com/docs/sdk/python/decorators).

In Goose, initialization requires certain environment variables to be present:

- `LANGFUSE_PUBLIC_KEY`: Your Langfuse public key
- `LANGFUSE_SECRET_KEY`: Your Langfuse secret key
- `LANGFUSE_BASE_URL`: The base URL of your Langfuse instance 

By default your local deployment and Goose will use the values in `env/.env.langfuse.local`.
