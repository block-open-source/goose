from exchange import Exchange, Message, Text
from exchange.content import Content
from exchange.providers import OpenAiProvider
from goose.toolkit.base import Toolkit, tool
from goose.utils.ask import ask_an_ai


class Reasoner(Toolkit):
    """Deep thinking toolkit for reasoning through problems and solutions"""

    def message_content(self, content: Content) -> Text:
        if isinstance(content, Text):
            return content
        else:
            return Text(str(content))

    @tool
    def deep_reason(self, problem: str) -> str:
        """
        Debug or reason about challenges or problems.
        It will take a minute to think about it and consider solutions.

        Args:
            problem (str): description of problem or errors seen.

        Returns:
            response (str): A solution, which may include a suggestion or code snippet.
        """
        # Create an instance of Exchange with the inlined OpenAI provider
        self.notifier.status("thinking...")
        provider = OpenAiProvider.from_env()

        # Create messages list
        existing_messages_copy = [
            Message(role=msg.role, content=[self.message_content(content) for content in msg.content])
            for msg in self.exchange_view.processor.messages
        ]
        exchange = Exchange(provider=provider, model="o1-preview", messages=existing_messages_copy, system=None)

        response = ask_an_ai(input="please help reason about this: " + problem, exchange=exchange, no_history=False)
        return response.content[0].text

    @tool
    def generate_code(self, instructions: str) -> str:
        """
        reason about and write code based on instructions given.
        this will consider and reason about the instructions and come up with code to solve it.

        Args:
            instructions (str): instructions of what code to write or how to modify it.

        Returns:
            response (str): generated code to be tested or applied. Not it will not write directly to files so you have to take it and process it if it is suitable.
        """  # noqa: E501
        # Create an instance of Exchange with the inlined OpenAI provider
        provider = OpenAiProvider.from_env()

        # clone messages, converting to text for context
        existing_messages_copy = [
            Message(role=msg.role, content=[self.message_content(content) for content in msg.content])
            for msg in self.exchange_view.processor.messages
        ]
        exchange = Exchange(provider=provider, model="o1-mini", messages=existing_messages_copy, system=None)

        self.notifier.status("generating code...")
        response = ask_an_ai(
            input="Please follow the instructions, "
            + "and ideally return relevant code and little commentary:"
            + instructions,
            exchange=exchange,
            no_history=False,
        )
        return response.content[0].text

    def system(self) -> str:
        """Retrieve instructions on how to use this reasoning and code generation tool"""
        return Message.load("prompts/reasoner.jinja").text
