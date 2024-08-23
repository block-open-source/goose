from unittest.mock import patch

import pytest
from goose.cli.config import ensure_config, read_config, session_path, write_config
from goose.profile import default_profile


@pytest.fixture
def mock_profile_config_path(tmp_path):
    with patch("goose.cli.config.PROFILES_CONFIG_PATH", tmp_path / "profiles.yaml") as mock_path:
        yield mock_path


@pytest.fixture
def mock_default_model_configuration():
    with patch(
        "goose.cli.config.default_model_configuration", return_value=("provider", "processor", "accelerator")
    ) as mock_default_model_configuration:
        yield mock_default_model_configuration


def test_read_write_config(mock_profile_config_path, profile_factory):
    profiles = {
        "profile1": profile_factory({"provider": "providerA"}),
    }
    write_config(profiles)

    assert read_config() == profiles


def test_ensure_config_create_profiles_file_with_default_profile(
    mock_profile_config_path, mock_default_model_configuration
):
    assert not mock_profile_config_path.exists()

    ensure_config(name="default")
    assert mock_profile_config_path.exists()

    assert read_config() == {"default": default_profile(*mock_default_model_configuration())}


def test_ensure_config_add_default_profile(mock_profile_config_path, profile_factory, mock_default_model_configuration):
    existing_profile = profile_factory({"provider": "providerA"})
    write_config({"profile1": existing_profile})

    ensure_config(name="default")

    assert read_config() == {
        "profile1": existing_profile,
        "default": default_profile(*mock_default_model_configuration()),
    }


@patch("goose.cli.config.Confirm.ask", return_value=True)
def test_ensure_config_overwrite_default_profile(
    mock_confirm, mock_profile_config_path, profile_factory, mock_default_model_configuration
):
    existing_profile = profile_factory({"provider": "providerA"})
    profile_name = "default"
    write_config({profile_name: existing_profile})

    expected_default_profile = default_profile(*mock_default_model_configuration())
    assert ensure_config(name="default") == expected_default_profile
    assert read_config() == {"default": expected_default_profile}


@patch("goose.cli.config.Confirm.ask", return_value=False)
def test_ensure_config_keep_original_default_profile(
    mock_confirm, mock_profile_config_path, profile_factory, mock_default_model_configuration
):
    existing_profile = profile_factory({"provider": "providerA"})
    profile_name = "default"
    write_config({profile_name: existing_profile})

    assert ensure_config(name="default") == existing_profile

    assert read_config() == {"default": existing_profile}


def test_session_path(mock_sessions_path):
    assert session_path("session1") == mock_sessions_path / "session1.jsonl"
