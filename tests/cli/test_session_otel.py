import pytest
from httpx import HTTPStatusError
from unittest.mock import MagicMock, patch
from exchange import Message, Text
from goose.cli.session import Session
from goose.cli.prompt.goose_prompt_session import GoosePromptSession
from goose.cli.prompt.user_input import PromptAction, UserInput
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter


@pytest.fixture
def memory_exporter():
    yield InMemorySpanExporter()


@pytest.fixture
def create_session_with_mock_configs(mock_sessions_path, exchange_factory, profile_factory, memory_exporter):
    # Create a tracer with the same memory exporter as test assertions use. We do this for each
    # test to ensure they can run in parallel without interfering with each other.
    tracer_provider = TracerProvider()
    memory_exporter = memory_exporter
    span_processor = SimpleSpanProcessor(memory_exporter)
    tracer_provider.add_span_processor(span_processor)

    with patch("goose.cli.session.build_exchange", return_value=exchange_factory()), patch(
        "goose.cli.session.load_profile", return_value=profile_factory()
    ), patch("goose.cli.session.SessionNotifier") as mock_session_notifier, patch(
        "goose.cli.session.load_provider", return_value="provider"
    ):
        mock_session_notifier.return_value = MagicMock()

        def create_session(session_attributes: dict = {}):
            return Session(**session_attributes, tracer=tracer_provider.get_tracer(__name__))

        yield create_session


def test_trace_run(create_session_with_mock_configs, memory_exporter):
    session = create_session_with_mock_configs()

    message = Message(role="user", id="abracadabra", content=[Text("List the files in this directory")])

    # Call the run function, for a single message which results in an exit.
    with patch.object(Session, "process_first_message", return_value=message), patch.object(
        Session, "reply", return_value=None
    ), patch.object(
        GoosePromptSession, "get_user_input", return_value=UserInput(action=PromptAction.EXIT)
    ), patch.object(Session, "save_session", return_value=None):
        session.run()

    spans = memory_exporter.get_finished_spans()
    assert len(spans) == 1
    span = spans[0]
    assert span.name == message.role
    assert span.attributes == {"goose.id": message.id, "goose.role": message.role, "goose.text": message.text}


def test_trace_run_interrupted(create_session_with_mock_configs, memory_exporter):
    session = create_session_with_mock_configs()

    message = Message(role="user", id="abracadabra", content=[Text("List the files in this directory")])

    # Call the run function, for a single message which results in an interrupt.
    with patch.object(Session, "process_first_message", return_value=message), patch.object(
        Session, "reply", side_effect=KeyboardInterrupt
    ), patch.object(
        GoosePromptSession, "get_user_input", return_value=UserInput(action=PromptAction.EXIT)
    ), patch.object(Session, "save_session", return_value=None):
        session.run()

    spans = memory_exporter.get_finished_spans()
    assert len(spans) == 1
    span = spans[0]
    assert span.name == message.role
    assert span.attributes == {"goose.id": message.id, "goose.role": message.role, "goose.text": message.text}
    assert span.status.is_ok is False
    assert span.status.description == "KeyboardInterrupt"


def test_trace_run_error(create_session_with_mock_configs, memory_exporter):
    session = create_session_with_mock_configs()

    message = Message(role="user", id="abracadabra", content=[Text("List the files in this directory")])

    # Call the run function, for a single message which results in an HTTP error.
    with patch.object(Session, "process_first_message", return_value=message), patch.object(
        Session, "reply", side_effect=HTTPStatusError("HTTP error", request=None, response=None)
    ), patch.object(
        GoosePromptSession, "get_user_input", return_value=UserInput(action=PromptAction.EXIT)
    ), patch.object(Session, "save_session", return_value=None):
        session.run()

    spans = memory_exporter.get_finished_spans()
    assert len(spans) == 1
    span = spans[0]
    assert span.name == message.role
    assert span.attributes == {"goose.id": message.id, "goose.role": message.role, "goose.text": message.text}
    assert span.status.is_ok is False
    assert span.status.description == "HTTP error"
