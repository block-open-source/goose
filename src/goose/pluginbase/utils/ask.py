from exchange import Exchange, Message, CheckpointData


def ask_an_ai(input: str, exchange: Exchange, prompt: str = "", no_history: bool = True) -> Message:
    """Sends a separate message to an LLM using a separate Exchange than the one underlying the Goose session.

    Can be used to summarize a file, or submit any other request that you'd like to an AI. The Exchange can have a
    history/prior context, or be wiped clean (by setting no_history to True).

    Parameters:
        input (str): The user's input string to be processed by the AI. Must be a non-empty string. Example: text from
            a file.
        exchange (Exchange): An object representing the AI exchange system which manages the state and flow of the
            conversation.
        prompt (str, optional): An optional new prompt to replace the current one in the exchange system. Defaults to
            None. Example: "Please summarize this file."
        no_history (bool, optional): A flag to determine if the conversation history should be cleared before
            processing the new input. True clears the context, False retains it. Defaults to True.

    Returns:
        reply (str): The AI's reply as a string.

    Raises:
        TypeError: If the `input` is not a non-empty string.
        Exception: If there is an issue within the exchange system, including errors from the provider or model.

    Example:
        # Create an instance of an Exchange system
        exchange_system = Exchange(provider=OpenAIProvider.from_env(), model="gpt-4")

        # Simulate asking the AI a question
        response = ask_an_ai("What is the weather today?", exchange_system)

        print(response)  # Outputs the AI's response to the question.
    """
    if no_history:
        exchange = clear_exchange(exchange)

    if prompt:
        exchange = replace_prompt(exchange, prompt)

    if not input:
        raise TypeError("`input` must be a string of finite length")

    msg = Message.user(input)
    exchange.add(msg)
    reply = exchange.reply()

    return reply


def clear_exchange(exchange: Exchange, clear_tools: bool = False) -> Exchange:
    """Clears the exchange object

    Args:
        exchange (Exchange): Exchange object to be overwritten. Messages and checkpoints are replaced with empty lists.
        clear_tools (bool): Boolean to indicate whether tools should be dropped from the exchange.

    Returns:
        new_exchange (Exchange)

    """
    if clear_tools:
        new_exchange = exchange.replace(messages=[], checkpoint_data=CheckpointData(), tools=())
    else:
        new_exchange = exchange.replace(messages=[], checkpoint_data=CheckpointData())
    return new_exchange


def replace_prompt(exchange: Exchange, prompt: str) -> Exchange:
    """Replaces the system prompt

    Args:
        exchange (Exchange): Exchange object to be overwritten. Messages and checkpoints are replaced with empty lists.
        prompt (str): The system prompt.

    Returns:
        new_exchange (Exchange)
    """

    new_exchange = exchange.replace(system=prompt)
    return new_exchange
