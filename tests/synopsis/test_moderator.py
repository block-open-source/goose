from unittest.mock import patch

import pytest
from exchange.content import Text
from exchange.message import Message
from goose.synopsis.moderator import Synopsis


@pytest.fixture
def mock_exchange(exchange_factory):
    exchange = exchange_factory()
    exchange.messages.extend(
        [
            Message(role="user", content=[Text("Test content for user message")]),
            Message(role="assistant", content=[Text("Test content for assistant message")]),
            Message(role="user", content=[Text("Another user message")]),
        ]
    )
    return exchange


def test_rewrite_with_tool_use(mock_exchange):
    tool_use_message = Message(role="user", content=[Text("Tool use message")])
    mock_exchange.messages.append(tool_use_message)

    with patch.object(Synopsis, "get_synopsis") as mock_get_synopsis:
        message = Message(role="synopsis", content=[Text("Updated synopsis")])
        mock_get_synopsis.return_value = message
        synopsis = Synopsis()
        synopsis.rewrite(mock_exchange)

        # The first message should be replaced, and the rest are cleared
        assert mock_exchange.messages == [message]
