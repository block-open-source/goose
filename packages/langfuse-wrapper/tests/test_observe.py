import pytest
from unittest.mock import patch, MagicMock
from langfuse_wrapper.langfuse_wrapper import observe_wrapper


def sample_function(x: int, y: int) -> int:
    return x + y


def sample_wrapped_function(x: int, y: int) -> int:
    return x


@pytest.fixture
def mock_langfuse_context():
    with patch("langfuse_wrapper.langfuse_wrapper.langfuse_context") as mock:
        yield mock


def test_observe_wrapper_with_credentials(mock_langfuse_context):
    with patch("langfuse_wrapper.langfuse_wrapper.HAS_LANGFUSE_CREDENTIALS", True):
        mock_observe = MagicMock(return_value=lambda f: sample_wrapped_function)
        mock_langfuse_context.observe = mock_observe

        decorated_function = observe_wrapper()(sample_function)
        result = decorated_function(2, 3)

        mock_observe.assert_called_once()
        assert result == 2


def test_observe_wrapper_with_credentials(mock_langfuse_context):
    with patch("langfuse_wrapper.langfuse_wrapper.HAS_LANGFUSE_CREDENTIALS", False):
        # Create a mock that returns a function that will return our sample_function
        mock_observe = MagicMock(return_value=lambda f: sample_wrapped_function)
        mock_langfuse_context.observe = mock_observe

        decorated_function = observe_wrapper()(sample_function)
        result = decorated_function(2, 3)

        #  test that if HAS_LANGFUSE_CREDENTIALS is false, the original function is returned
        mock_observe.assert_not_called()
        assert result == 5
