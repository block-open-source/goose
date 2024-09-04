from unittest.mock import patch

import pytest
from goose._internal.profile import default_profile
from goose._internal.profile.config import _ensure_config, _read_config, _write_config


@pytest.fixture
def mock_profile_config_path(tmp_path):
    with patch("goose._internal.profile.config.PROFILES_CONFIG_PATH", tmp_path / "profiles.yaml") as mock_path:
        yield mock_path


@pytest.fixture
def mock_default_model_configuration():
    with patch(
        "goose._internal.profile.config._default_model_configuration", return_value=("provider", "processor", "accelerator")
    ) as mock_default_model_configuration:
        yield mock_default_model_configuration


def test_read_write_config(mock_profile_config_path, profile_factory):
    profiles = {
        "profile1": profile_factory({"provider": "providerA"}),
    }
    _write_config(profiles)

    assert _read_config() == profiles

def test_ensure_config_create_profiles_file_with_default_profile(
    mock_profile_config_path, mock_default_model_configuration
):
    assert not mock_profile_config_path.exists()

    _ensure_config(name="default")
    assert mock_profile_config_path.exists()

    assert _read_config() == {"default": default_profile(*mock_default_model_configuration())}

@patch("goose._internal.profile.config.print")
def test_ensure_config_add_default_profile(mock_profile_config_path, profile_factory, mock_default_model_configuration):
    existing_profile = profile_factory({"provider": "providerA"})
    _write_config({"profile1": existing_profile})

    _ensure_config(name="default")

    assert _read_config() == {
        "profile1": existing_profile,
        "default": default_profile(*mock_default_model_configuration()),
    }


@patch("goose._internal.profile.config.Confirm.ask", return_value=True)
@patch("goose._internal.profile.config.print")
def test_ensure_config_overwrite_default_profile(
    mock_confirm, mock_profile_config_path, profile_factory, mock_default_model_configuration
):
    existing_profile = profile_factory({"provider": "providerA"})
    profile_name = "default"
    _write_config({profile_name: existing_profile})

    expected_default_profile = default_profile(*mock_default_model_configuration())
    assert _ensure_config(name="default") == expected_default_profile
    assert _read_config() == {"default": expected_default_profile}


@patch("goose._internal.profile.config.Confirm.ask", return_value=False)
@patch("goose._internal.profile.config.print")
def test_ensure_config_keep_original_default_profile(
    mock_confirm, mock_profile_config_path, profile_factory, mock_default_model_configuration
):
    existing_profile = profile_factory({"provider": "providerA"})
    profile_name = "default"
    _write_config({profile_name: existing_profile})

    assert _ensure_config(name="default") == existing_profile

    assert _read_config() == {"default": existing_profile}

