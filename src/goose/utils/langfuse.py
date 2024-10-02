from langfuse.decorators import langfuse_context


def setup_langfuse(use_langfuse: bool = False) -> None:
    if use_langfuse:
        try:
            assert langfuse_context.auth_check()
            langfuse_context.configure(enabled=True)
        except Exception as e:
            print("No Langfuse or Langfuse credentials found:\n", e)
    else:
        langfuse_context.configure(enabled=False)
