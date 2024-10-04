from unittest.mock import MagicMock
from exchange.exchange import Exchange
from exchange.message import Message
from exchange.moderators.passive import PassiveModerator
from exchange.providers.base import Provider
from exchange.tool import Tool
from exchange.token_usage_collector import _TokenUsageCollector

MODEL_NAME = "test-model"


def create_exchange(mock_provider, dummy_tool):
    return Exchange(
        provider=mock_provider,
        model=MODEL_NAME,
        system="test-system",
        tools=(Tool.from_function(dummy_tool),),
        messages=[],
        moderator=PassiveModerator(),
    )


def test_exchange_generate_collect_usage(usage_factory, dummy_tool, monkeypatch):
    mock_provider = MagicMock(spec=Provider)
    mock_usage_collector = MagicMock(spec=_TokenUsageCollector)
    usage = usage_factory()
    mock_provider.complete.return_value = (Message.assistant("msg"), usage)
    exchange = create_exchange(mock_provider, dummy_tool)

    monkeypatch.setattr("exchange.exchange._token_usage_collector", mock_usage_collector)
    exchange.generate()

    mock_usage_collector.collect.assert_called_once_with(MODEL_NAME, usage)
