import os
from typing import Type

import httpx

from exchange.providers.openai import OpenAiProvider

OLLAMA_HOST = "http://localhost:11434/"
OLLAMA_MODEL = "mistral-nemo"


class OllamaProvider(OpenAiProvider):
    """Provides chat completions for models hosted by Ollama"""

    __doc__ += f"""

Here's an example profile configuration to try:

    ollama:
      provider: ollama
      processor: {OLLAMA_MODEL}
      accelerator: {OLLAMA_MODEL}
      moderator: passive
      toolkits:
      - name: developer
        requires: {{}}
"""

    def __init__(self, client: httpx.Client) -> None:
        print("PLEASE NOTE: the ollama provider is experimental, use with care")
        super().__init__(client)

    @classmethod
    def from_env(cls: Type["OllamaProvider"]) -> "OllamaProvider":
        ollama_url = os.environ.get("OLLAMA_HOST", OLLAMA_HOST)
        timeout = httpx.Timeout(60 * 10)

        # from_env is expected to fail if required ENV variables are not
        # available. Since this provider can run with defaults, we substitute
        # an Ollama health check (GET /) to determine if the service is ok.
        httpx.get(ollama_url, timeout=timeout)

        # When served by Ollama, the OpenAI API is available at the path "v1/".
        client = httpx.Client(base_url=ollama_url + "v1/", timeout=timeout)
        return cls(client)
