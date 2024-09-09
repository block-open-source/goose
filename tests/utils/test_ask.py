from unittest.mock import MagicMock, patch

import pytest
from exchange import Exchange, CheckpointData
from goose.utils.ask import ask_an_ai, clear_exchange, replace_prompt


# tests for `ask_an_ai`
def test_ask_an_ai_empty_input():
    """Test that function raises TypeError if input is empty."""
    exchange = MagicMock(spec=Exchange)
    with pytest.raises(TypeError):
        ask_an_ai("", exchange)


def test_ask_an_ai_no_history():
    """Test the no_history functionality."""
    exchange = MagicMock(spec=Exchange)
    with patch("goose.utils.ask.clear_exchange") as mock_clear:
        ask_an_ai("Test input", exchange, no_history=True)
        mock_clear.assert_called_once_with(exchange)


def test_ask_an_ai_prompt_replacement():
    """Test that the prompt is replaced if provided."""
    exchange = MagicMock(spec=Exchange)
    prompt = "New prompt"

    with patch("goose.utils.ask.replace_prompt") as mock_replace_prompt:
        # Configure the mock to return a new mock object with the same spec
        modified_exchange = MagicMock(spec=Exchange)
        mock_replace_prompt.return_value = modified_exchange

        ask_an_ai("Test input", exchange, prompt=prompt, no_history=False)

        # Check if replace_prompt was called correctly
        mock_replace_prompt.assert_called_once_with(exchange, prompt)

        # Assert that the modified exchange was returned correctly
        assert mock_replace_prompt.return_value is modified_exchange, "Should return the modified exchange mock"


def test_ask_an_ai_exchange_usage():
    """Test that the exchange adds and processes the message correctly."""
    exchange = MagicMock(spec=Exchange)
    input_text = "Test input"
    message_mock = MagicMock(return_value="Mocked Message")

    with patch("goose.utils.ask.Message.user", new=message_mock):
        ask_an_ai(input_text, exchange, no_history=False)

        # Assert that Message.user was called with the correct input
        message_mock.assert_called_once_with(input_text)

        # Assert that exchange.add was called with the mocked message
        exchange.add.assert_called_once_with("Mocked Message")
        exchange.reply.assert_called_once()


def test_ask_an_ai_return_value():
    """Test that the function returns the correct reply."""
    exchange = MagicMock(spec=Exchange)
    expected_reply = "AI response"
    exchange.reply.return_value = expected_reply
    result = ask_an_ai("Test input", exchange, no_history=False)
    assert result == expected_reply, "Function should return the reply from the exchange."


# tests for `clear_exchange`
def test_clear_exchange_without_tools():
    """Test clearing messages and checkpoints but not tools."""
    # Arrange
    exchange = MagicMock(spec=Exchange)

    # Act
    new_exchange = clear_exchange(exchange, clear_tools=False)

    # Assert
    exchange.replace.assert_called_once_with(messages=[], checkpoint_data=CheckpointData())
    assert new_exchange == exchange.replace.return_value, "Should return the modified exchange"


def test_clear_exchange_with_tools():
    """Test clearing messages, checkpoints, and tools."""
    # Arrange
    exchange = MagicMock(spec=Exchange)

    # Act
    new_exchange = clear_exchange(exchange, clear_tools=True)

    # Assert
    exchange.replace.assert_called_once_with(messages=[], checkpoint_data=CheckpointData(), tools=())
    assert new_exchange == exchange.replace.return_value, "Should return the modified exchange with tools cleared"


def test_clear_exchange_return_value():
    """Test that the returned value is a new exchange object."""
    # Arrange
    exchange = MagicMock(spec=Exchange)
    new_exchange_mock = MagicMock(spec=Exchange)
    exchange.replace.return_value = new_exchange_mock

    # Act
    new_exchange = clear_exchange(exchange, clear_tools=False)

    # Assert
    assert new_exchange == new_exchange_mock, "Returned exchange should be the new exchange instance"


# tests for `replace_prompt`
def test_replace_prompt():
    """Test that the system prompt is correctly replaced."""
    # Arrange
    exchange = MagicMock(spec=Exchange)
    prompt = "New system prompt"

    # Act
    new_exchange = replace_prompt(exchange, prompt)

    # Assert
    exchange.replace.assert_called_once_with(system=prompt)
    assert new_exchange == exchange.replace.return_value, "Should return the modified exchange with the new prompt"


def test_replace_prompt_return_value():
    """Test that the returned value is a new exchange object."""
    # Arrange
    exchange = MagicMock(spec=Exchange)
    expected_new_exchange = MagicMock(spec=Exchange)
    exchange.replace.return_value = expected_new_exchange

    # Act
    new_exchange = replace_prompt(exchange, "Another prompt")

    # Assert
    assert new_exchange == expected_new_exchange, "Returned exchange should be the new exchange instance"
