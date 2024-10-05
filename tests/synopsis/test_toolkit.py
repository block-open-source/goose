import pytest
from unittest.mock import patch, MagicMock
from goose.synopsis.toolkit import SynopsisDeveloper
from goose.synopsis.system import system

@pytest.fixture
def mock_notifier():
    return MagicMock()

@pytest.fixture
def developer(mock_notifier):
    return SynopsisDeveloper(notifier=mock_notifier)

def test_system(developer):
    with patch('goose.synopsis.toolkit.Message.load') as mock_load:
        mock_load.return_value.text = "System prompt"
        result = developer.system()
        assert result == "System prompt"

def test_logshell(developer):
    developer.logshell("ls -l", "test")
    developer.notifier.log.assert_called()

@patch('goose.synopsis.toolkit.shell')
def test_source(mock_shell, developer):
    mock_shell.return_value = "VAR1=value1\nVAR2=value2"
    result = developer.source("test.sh")
    assert result == "Sourced test.sh"
    assert system.env["VAR1"] == "value1"
    assert system.env["VAR2"] == "value2"

@patch('goose.synopsis.toolkit.shell')
def test_shell(mock_shell, developer):
    mock_shell.return_value = "Command output"
    result = developer.shell("ls -l")
    assert result == "Command output"

@patch('goose.synopsis.toolkit.system')
def test_read_file(mock_system, developer):
    mock_system.to_patho.return_value.read_text.return_value = "File content"
    result = developer.read_file("test.txt")
    assert "The file content at test.txt has been updated above." in result

@patch('goose.synopsis.toolkit.system')
def test_write_file(mock_system, developer):
    result = developer.write_file("test.txt", "New content")
    assert result == "Successfully wrote to test.txt"

@patch('goose.synopsis.toolkit.system')
def test_patch_file(mock_system, developer):
    mock_system.to_patho.return_value.read_text.return_value = "Old content"
    result = developer.patch_file("test.txt", "Old", "New")
    assert result == "Succesfully replaced before with after."

@patch('goose.synopsis.toolkit.subprocess.Popen')
@patch('goose.synopsis.toolkit.system')
def test_start_process(mock_system, mock_popen, developer):
    mock_system.add_process.return_value = 1234
    result = developer.start_process("python script.py")
    assert result == 1234

@patch('goose.synopsis.toolkit.system')
def test_list_processes(mock_system, developer):
    mock_system.get_processes.return_value = {1: "process1", 2: "process2"}
    result = developer.list_processes()
    assert result == {1: "process1", 2: "process2"}

@patch('goose.synopsis.toolkit.system')
def test_view_process_output(mock_system, developer):
    mock_system.view_process_output.return_value = "Process output"
    result = developer.view_process_output(1234)
    assert result == "Process output"

@patch('goose.synopsis.toolkit.system')
def test_cancel_process(mock_system, developer):
    mock_system.cancel_process.return_value = True
    result = developer.cancel_process(1234)
    assert result == "process 1234 cancelled"

@patch('goose.synopsis.toolkit.system')
def test_change_dir(mock_system, developer):
    mock_system.to_patho.return_value.is_dir.return_value = True
    mock_system.to_patho.return_value.resolve.return_value = MagicMock()
    mock_system.to_patho.return_value.resolve.return_value.__lt__.return_value = False
    result = developer.change_dir("new_dir")
    assert result == "new_dir"
    mock_system.cwd = "new_dir"
    assert mock_system.cwd == "new_dir"