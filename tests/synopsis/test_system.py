import os
import pytest
from unittest.mock import patch, MagicMock
from pathlib import Path
from goose.synopsis.system import OperatingSystem, File


@pytest.fixture
def os_instance():
    return OperatingSystem()


def test_file_context():
    file = File(path="test.py", content="print('Hello')", language="python")
    expected = "test.py\n```python\nprint('Hello')\n```"
    assert file.context == expected


def test_operating_system_init(os_instance):
    assert isinstance(os_instance.cwd, str)
    assert os_instance._active_files == set()
    assert os_instance._processes == {}


def test_to_relative(os_instance):
    with patch.object(os_instance, 'cwd', '/home/user'):
        assert os_instance.to_relative('/home/user/test.py') == 'test.py'
        assert os_instance.to_relative('/home/other/test.py') == '../other/test.py'


def test_to_patho(os_instance):
    with patch.object(os_instance, 'cwd', '/home/user'):
        result = os_instance.to_patho('test.py')
        assert result.name == 'test.py'
        assert result.parent.name == 'user'


def test_remember_forget_file(os_instance):
    os_instance.remember_file('test.py')
    assert os_instance.is_active('test.py')
    os_instance.forget_file('test.py')
    assert not os_instance.is_active('test.py')


@patch('subprocess.Popen')
def test_add_process(mock_popen, os_instance):
    mock_process = MagicMock()
    mock_process.pid = 1234
    mock_popen.return_value = mock_process
    
    process_id = os_instance.add_process(mock_process)
    assert process_id == 1234
    assert 1234 in os_instance._processes


def test_get_processes(os_instance):
    os_instance._processes = {1: MagicMock(args='cmd1'), 2: MagicMock(args='cmd2')}
    processes = os_instance.get_processes()
    assert processes == {1: 'cmd1', 2: 'cmd2'}


@patch('subprocess.Popen')
def test_cancel_process(mock_popen, os_instance):
    mock_process = MagicMock()
    os_instance._processes = {1234: mock_process}
    
    assert os_instance.cancel_process(1234)
    mock_process.terminate.assert_called_once()
    assert 1234 not in os_instance._processes

    assert not os_instance.cancel_process(5678)  # Non-existent process
