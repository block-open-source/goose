import sys
import unittest.mock as mock

from goose.utils.autocomplete import SUPPORTED_SHELLS, is_autocomplete_installed, setup_autocomplete


def test_supported_shells():
    assert SUPPORTED_SHELLS == ["bash", "zsh", "fish"]


def test_install_autocomplete(tmp_path):
    file = tmp_path / "test_bash_autocomplete"
    assert is_autocomplete_installed(file) is False

    file.write_text("_GOOSE_COMPLETE")
    assert is_autocomplete_installed(file) is True


@mock.patch("sys.stdout")
def test_setup_bash(mocker: mock.MagicMock):
    setup_autocomplete("bash", install=False)
    sys.stdout.write.assert_called_with('eval "$(_GOOSE_COMPLETE=bash_source goose)"\n')


@mock.patch("sys.stdout")
def test_setup_zsh(mocker: mock.MagicMock):
    setup_autocomplete("zsh", install=False)
    sys.stdout.write.assert_called_with('eval "$(_GOOSE_COMPLETE=zsh_source goose)"\n')


@mock.patch("sys.stdout")
def test_setup_fish(mocker: mock.MagicMock):
    setup_autocomplete("fish", install=False)
    sys.stdout.write.assert_called_with("_GOOSE_COMPLETE=fish_source goose | source\n")
