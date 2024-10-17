import pytest
from unittest.mock import patch, MagicMock
from exchange.langfuse_wrapper import observe_wrapper


@pytest.fixture
def mock_langfuse_context():
    with patch("exchange.langfuse_wrapper.langfuse_context") as mock:
        yield mock


@patch("exchange.langfuse_wrapper.HAS_LANGFUSE_CREDENTIALS", True)
def test_function_is_wrapped(mock_langfuse_context):
    mock_observe = MagicMock(side_effect=lambda *args, **kwargs: lambda fn: fn)
    mock_langfuse_context.observe = mock_observe

    def original_function(x: int, y: int) -> int:
        return x + y

    # test function before we decorate it with
    # @observe_wrapper("arg1", kwarg1="kwarg1")
    assert not hasattr(original_function, "__wrapped__")

    # ensure we args get passed along (e.g. @observe(capture_input=False, capture_output=False))
    decorated_function = observe_wrapper("arg1", kwarg1="kwarg1")(original_function)
    assert hasattr(decorated_function, "__wrapped__")
    assert decorated_function.__wrapped__ is original_function, "Function is not properly wrapped"

    assert decorated_function(2, 3) == 5
    mock_observe.assert_called_once()
    mock_observe.assert_called_with("arg1", kwarg1="kwarg1")


@patch("exchange.langfuse_wrapper.HAS_LANGFUSE_CREDENTIALS", False)
def test_function_is_not_wrapped(mock_langfuse_context):
    mock_observe = MagicMock(return_value=lambda f: f)
    mock_langfuse_context.observe = mock_observe

    @observe_wrapper("arg1", kwarg1="kwarg1")
    def hello() -> str:
        return "Hello"

    assert not hasattr(hello, "__wrapped__")
    assert hello() == "Hello"

    mock_observe.assert_not_called()
