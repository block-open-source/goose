<p align="center">
<a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="#example">Example</a> •
  <a href="#plugins">Plugins</a>
</p>

<p align="center"><strong>Exchange</strong> <em>- a uniform python SDK for message generation with LLMs</em></p>

- Provides a flexible layer for message handling and generation
- Directly integrates python functions into tool calling
- Persistently surfaces errors to the underlying models to support reflection

## Example

> [!NOTE]
> Before you can run this example, you need to setup an API key with
> `export OPENAI_API_KEY=your-key-here`

``` python
from exchange import Exchange, Message, Tool
from exchange.providers import OpenAiProvider

def word_count(text: str):
    """Get the count of words in text

    Args:
        text (str): The text with words to count
    """
    return len(text.split(" "))

ex = Exchange(
    provider=OpenAiProvider.from_env(),
    model="gpt-4o",
    system="You are a helpful assistant.",
    tools=[Tool.from_function(word_count)],
)
ex.add(Message.user("Count the number of words in this current message"))

# The model sees it has a word count tool, and should use it along the way to answer
# This will call all the tools as needed until the model replies with the final result
reply = ex.reply()
print(reply.text)

# you can see all the tool calls in the message history
print(ex.messages)
```

## Plugins

*exchange* has a plugin mechanism to add support for additional providers and moderators. If you need a 
provider not supported here, we'd be happy to review [contributions][CONTRIBUTING]. But you
can also consider building and using your own plugin. 

To create a `Provider` plugin, subclass `exchange.provider.Provider`. You will need to 
implement the `complete` method. For example this is what we use as a mock in our tests.
You can see a full implementation example of the [OpenAiProvider][openaiprovider]. We
also generally recommend implementing a `from_env` classmethod to instantiate the provider.

``` python
class MockProvider(Provider):
    def __init__(self, sequence: List[Message]):
        # We'll use init to provide a preplanned reply sequence
        self.sequence = sequence
        self.call_count = 0

    def complete(
        self, model: str, system: str, messages: List[Message], tools: List[Tool]
    ) -> Message:
        output = self.sequence[self.call_count]
        self.call_count += 1
        return output
```

Then use [python packaging's entrypoints][plugins] to register your plugin. 

``` toml
[project.entry-points.'exchange.provider']
example = 'path.to.plugin:ExampleProvider'
```

Your plugin will then be available in your application or other applications built on *exchange*
through:

``` python
from exchange.providers import get_provider

provider = get_provider('example').from_env()
```

[CONTRIBUTING]: CONTRIBUTING.md
[openaiprovider]: src/exchange/providers/openai.py
[plugins]: https://packaging.python.org/en/latest/guides/creating-and-discovering-plugins/
