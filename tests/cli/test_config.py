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


def test_ensure_config_create_profiles_file_with_default_profile_with_name_default(
    mock_profile_config_path, mock_default_model_configuration
):
    assert not mock_profile_config_path.exists()

    (profile_name, profile) = ensure_config(name=None)

    expected_profile = default_profile(*mock_default_model_configuration())

    assert profile_name == "default"
    assert profile == expected_profile
    assert mock_profile_config_path.exists()
    assert read_config() == {"default": expected_profile}


def test_ensure_config_create_profiles_file_with_default_profile_with_profile_name(
    mock_profile_config_path, mock_default_model_configuration
):
    assert not mock_profile_config_path.exists()

    (profile_name, profile) = ensure_config(name="my_profile")

    expected_profile = default_profile(*mock_default_model_configuration())

    assert profile_name == "my_profile"
    assert profile == expected_profile
    assert mock_profile_config_path.exists()
    assert read_config() == {"my_profile": expected_profile}


def test_ensure_config_add_default_profile_when_profile_not_exist(
    mock_profile_config_path, profile_factory, mock_default_model_configuration
):
    existing_profile = profile_factory({"provider": "providerA"})
    write_config({"profile1": existing_profile})

    (profile_name, new_profile) = ensure_config(name="my_new_profile")

    expected_profile = default_profile(*mock_default_model_configuration())
    assert profile_name == "my_new_profile"
    assert new_profile == expected_profile
    assert read_config() == {
        "profile1": existing_profile,
        "my_new_profile": expected_profile,
    }


def test_ensure_config_get_existing_profile_not_exist(
    mock_profile_config_path, profile_factory, mock_default_model_configuration
):
    existing_profile = profile_factory({"provider": "providerA"})
    write_config({"profile1": existing_profile})

    (profile_name, profile) = ensure_config(name="profile1")

    assert profile_name == "profile1"
    assert profile == existing_profile
    assert read_config() == {
        "profile1": existing_profile,
    }


def test_session_path(mock_sessions_path):
    assert session_path("session1") == mock_sessions_path / "session1.jsonl"
