from unittest.mock import MagicMock, patch

import pytest
from goose.notifier import Notifier
from goose.toolkit.developer import Developer
from goose.toolkit.language_server import LanguageServerCoordinator
from goose.utils.language import Language

TEST_NOTIFIER = MagicMock(spec=Notifier)


@pytest.fixture
def language_server_toolkit():
    developer_toolkit = Developer(notifier=TEST_NOTIFIER)
    return LanguageServerCoordinator(
        notifier=TEST_NOTIFIER, requires=dict(developer=developer_toolkit), prompt_user_to_start=False
    )


def test_singleton_language_server(language_server_toolkit):
    developer_toolkit = Developer(notifier=TEST_NOTIFIER)
    lsc_two = LanguageServerCoordinator(
        notifier=TEST_NOTIFIER, requires=dict(developer=developer_toolkit), prompt_user_to_start=False
    )

    return language_server_toolkit is lsc_two


def test_server_context(language_server_toolkit):
    with language_server_toolkit.language_server_client.start_servers() as servers:
        assert len(servers.language_servers[Language.PYTHON]) > 0
        assert servers.server_threads["JediServer"].daemon
        assert servers.server_threads["JediServer"].is_alive()
        assert servers.client_loops["JediServer"].is_running

    assert not servers.server_threads["JediServer"].is_alive()
    assert servers.client_loops["JediServer"].is_closed


def test_request_definition(language_server_toolkit):
    with language_server_toolkit.language_server_client.start_servers() as _:
        result = language_server_toolkit.request_definition(__file__, 15, 28)
    assert "goose/tests/toolkit/test_language_server.py" in result["results"][0]
    assert "TEST_NOTIFIER = MagicMock(spec=Notifier)" in result["results"][0]
    assert result["current_page_number"] == 1
    assert result["total_pages"] == 1


def test_invalid_definition_requested(language_server_toolkit):
    try:
        with language_server_toolkit.language_server_client.start_servers() as _:
            language_server_toolkit.request_definition(__file__, 1000, 1000)
    except Exception as e:
        assert e.args[0] == "ValueError: `line` parameter is not in a valid range."


def test_ensure_language_server_client_is_none_if_no_language_servers_exist():
    with patch("goose.toolkit.language_server.load_plugins", return_value=dict()):
        developer_toolkit = Developer(notifier=TEST_NOTIFIER)
        language_server_toolkit = LanguageServerCoordinator(
            notifier=TEST_NOTIFIER, requires=dict(developer=developer_toolkit), prompt_user_to_start=False
        )
        assert language_server_toolkit.language_server_client is None


def test_request_definition_for_unsupported_language(language_server_toolkit):
    with language_server_toolkit.language_server_client.start_servers() as _:
        with pytest.raises(ValueError) as e:
            language_server_toolkit.request_definition("some/file.rs", 17, 28)
        assert str(e.value) == "Unsupported language for file some/file.rs"
