from unittest.mock import MagicMock

import pytest
from goose.language_server.core.exception import LanguageServerError
from goose.language_server.core.server import Error
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
        assert servers.loop_threads["JediServer"].daemon
        assert servers.loop_threads["JediServer"].is_alive()
        assert servers.server_loops["JediServer"].is_running

    assert not servers.loop_threads["JediServer"].is_alive()
    assert servers.server_loops["JediServer"].is_closed
    LanguageServerCoordinator._instance = None


def test_request_definition(language_server_toolkit):
    with language_server_toolkit.language_server_client.start_servers() as _:
        result = language_server_toolkit.request_definition(__file__, 17, 28)
    assert "goose/tests/toolkit/test_language_server.py" in result["results"][0]
    assert "TEST_NOTIFIER = MagicMock(spec=Notifier)" in result["results"][0]
    assert result["current_page_number"] == 1
    assert result["total_pages"] == 1
