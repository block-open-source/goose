import json
import os
from time import time
from unittest.mock import Mock, patch

import pytest
from exchange import Exchange
from goose.profile import Profile


@pytest.fixture
def profile_factory():
    def _create_profile(attributes={}):
        profile_attrs = {
            "provider": "mock_provider",
            "processor": "mock_processor",
            "accelerator": "mock_accelerator",
            "moderator": "mock_moderator",
            "toolkits": [],
        }
        profile_attrs.update(attributes)
        return Profile(**profile_attrs)

    return _create_profile


@pytest.fixture
def exchange_factory():
    def _create_exchange(attributes={}):
        exchange_attrs = {
            "provider": "mock_provider",
            "system": "mock_system",
            "tools": [],
            "moderator": Mock(),
            "model": "mock_model",
        }
        exchange_attrs.update(attributes)
        return Exchange(**exchange_attrs)

    return _create_exchange


@pytest.fixture
def mock_sessions_path(tmp_path):
    with patch("goose.cli.config.SESSIONS_PATH", tmp_path) as mock_path:
        yield mock_path


@pytest.fixture
def create_session_file():
    def _create_session_file(messages, session_file_path, mtime=time()):
        with open(session_file_path, "w") as session_file:
            for m in messages:
                json.dump(m.to_dict(), session_file)
                session_file.write("\n")
        session_file.close()
        os.utime(session_file_path, (mtime, mtime))

    return _create_session_file
