from unittest.mock import MagicMock, patch

import pytest
from exchange import Message, ToolUse, ToolResult
from goose.cli.prompt.goose_prompt_session import GoosePromptSession
from goose.cli.prompt.user_input import PromptAction, UserInput
from goose.cli.session import Session
from prompt_toolkit import PromptSession

SPECIFIED_SESSION_NAME = "mySession"
SESSION_NAME = "test"


@pytest.fixture
def mock_specified_session_name():
    with patch.object(PromptSession, "prompt", return_value=SPECIFIED_SESSION_NAME) as specified_session_name:
        yield specified_session_name


@pytest.fixture
def create_session_with_mock_configs(mock_sessions_path, exchange_factory, profile_factory):
    with patch("goose.cli.session.build_exchange", return_value=exchange_factory()), patch(
        "goose.cli.session.load_profile", return_value=profile_factory()
    ), patch("goose.cli.session.SessionNotifier") as mock_session_notifier, patch(
        "goose.cli.session.load_provider", return_value="provider"
    ):
        mock_session_notifier.return_value = MagicMock()

        def create_session(session_attributes: dict = {}):
            return Session(**session_attributes)

        yield create_session


def test_session_does_not_extend_last_user_text_message_on_init(
    create_session_with_mock_configs, mock_sessions_path, create_session_file
):
    messages = [Message.user("Hello"), Message.assistant("Hi"), Message.user("Last should be removed")]
    create_session_file(messages, mock_sessions_path / f"{SESSION_NAME}.jsonl")

    session = create_session_with_mock_configs({"name": SESSION_NAME})
    print("Messages after session init:", session.exchange.messages)  # Debugging line
    assert len(session.exchange.messages) == 2
    assert [message.text for message in session.exchange.messages] == ["Hello", "Hi"]


def test_session_adds_resume_message_if_last_message_is_tool_result(
    create_session_with_mock_configs, mock_sessions_path, create_session_file
):
    messages = [
        Message.user("Hello"),
        Message(role="assistant", content=[ToolUse(id="1", name="first_tool", parameters={})]),
        Message(role="user", content=[ToolResult(tool_use_id="1", output="output")]),
    ]
    create_session_file(messages, mock_sessions_path / f"{SESSION_NAME}.jsonl")

    session = create_session_with_mock_configs({"name": SESSION_NAME})
    print("Messages after session init:", session.exchange.messages)  # Debugging line
    assert len(session.exchange.messages) == 4
    assert session.exchange.messages[-1].role == "assistant"
    assert session.exchange.messages[-1].text == "I see we were interrupted. How can I help you?"


def test_session_removes_tool_use_and_adds_resume_message_if_last_message_is_tool_use(
    create_session_with_mock_configs, mock_sessions_path, create_session_file
):
    messages = [
        Message.user("Hello"),
        Message(role="assistant", content=[ToolUse(id="1", name="first_tool", parameters={})]),
    ]
    create_session_file(messages, mock_sessions_path / f"{SESSION_NAME}.jsonl")

    session = create_session_with_mock_configs({"name": SESSION_NAME})
    print("Messages after session init:", session.exchange.messages)  # Debugging line
    assert len(session.exchange.messages) == 2
    assert [message.text for message in session.exchange.messages] == [
        "Hello",
        "I see we were interrupted. How can I help you?",
    ]


def test_save_session_create_session(mock_sessions_path, create_session_with_mock_configs, mock_specified_session_name):
    session = create_session_with_mock_configs()
    session.exchange.messages.append(Message.assistant("Hello"))

    session.save_session()
    session_file = mock_sessions_path / f"{SPECIFIED_SESSION_NAME}.jsonl"
    assert session_file.exists()

    saved_messages = session.load_session()
    assert len(saved_messages) == 1
    assert saved_messages[0].text == "Hello"


def test_save_session_resume_session_new_file(
    mock_sessions_path, create_session_with_mock_configs, mock_specified_session_name, create_session_file
):
    with patch("goose.cli.session.confirm", return_value=False):
        existing_messages = [Message.assistant("existing_message")]
        existing_session_file = mock_sessions_path / f"{SESSION_NAME}.jsonl"
        create_session_file(existing_messages, existing_session_file)

        new_session_file = mock_sessions_path / f"{SPECIFIED_SESSION_NAME}.jsonl"
        assert not new_session_file.exists()

        session = create_session_with_mock_configs({"name": SESSION_NAME})
        session.exchange.messages.append(Message.assistant("new_message"))

        session.save_session()

        assert new_session_file.exists()
        assert existing_session_file.exists()

        saved_messages = session.load_session()
        assert [message.text for message in saved_messages] == ["existing_message", "new_message"]


def test_save_session_resume_session_existing_session_file(
    mock_sessions_path, create_session_with_mock_configs, create_session_file
):
    with patch("goose.cli.session.confirm", return_value=True):
        existing_messages = [Message.assistant("existing_message")]
        existing_session_file = mock_sessions_path / f"{SESSION_NAME}.jsonl"
        create_session_file(existing_messages, existing_session_file)

        session = create_session_with_mock_configs({"name": SESSION_NAME})
        session.exchange.messages.append(Message.assistant("new_message"))

        session.save_session()

        saved_messages = session.load_session()
        assert [message.text for message in saved_messages] == ["existing_message", "new_message"]


def test_process_first_message_return_message(create_session_with_mock_configs):
    session = create_session_with_mock_configs()
    with patch.object(
        GoosePromptSession, "get_user_input", return_value=UserInput(action=PromptAction.CONTINUE, text="Hello")
    ):
        message = session.process_first_message()

        assert message.text == "Hello"
        assert len(session.exchange.messages) == 0


def test_process_first_message_to_exit(create_session_with_mock_configs):
    session = create_session_with_mock_configs()
    with patch.object(GoosePromptSession, "get_user_input", return_value=UserInput(action=PromptAction.EXIT)):
        message = session.process_first_message()

        assert message is None


def test_process_first_message_return_last_exchange_message(create_session_with_mock_configs):
    session = create_session_with_mock_configs()
    session.exchange.messages.append(Message.user("Hi"))

    message = session.process_first_message()

    assert message.text == "Hi"
    assert len(session.exchange.messages) == 0


def test_generate_session_name(create_session_with_mock_configs):
    session = create_session_with_mock_configs()
    with patch.object(GoosePromptSession, "get_save_session_name", return_value=SPECIFIED_SESSION_NAME):
        session.generate_session_name()

        assert session.name == SPECIFIED_SESSION_NAME
