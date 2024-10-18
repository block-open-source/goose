import os
from unittest.mock import Mock
import pytest
from goose.synopsis.system import OperatingSystem


@pytest.fixture
def os_instance(tmpdir):
    original_cwd = os.getcwd()
    os.chdir(tmpdir)
    yield OperatingSystem(cwd=str(tmpdir))
    os.chdir(original_cwd)


def test_to_relative(os_instance, tmpdir):
    abs_path = os.path.join(tmpdir, "test_file.txt")
    rel_path = os_instance.to_relative(abs_path)
    assert rel_path == "test_file.txt"


def test_remember_forget_file(os_instance, tmpdir):
    test_file = tmpdir.join("test_file.txt")
    test_file.write("test content")

    os_instance.remember_file(str(test_file))
    assert os_instance.is_active(str(test_file))

    os_instance.forget_file(str(test_file))
    assert not os_instance.is_active(str(test_file))


def test_active_files(os_instance, tmpdir):
    test_file1 = tmpdir.join("test_file1.txt")
    test_file2 = tmpdir.join("test_file2.py")
    test_file1.write("test content 1")
    test_file2.write("test content 2")

    os_instance.remember_file(str(test_file1))
    os_instance.remember_file(str(test_file2))

    active_files = list(os_instance.active_files)
    assert len(active_files) == 2
    assert any(f.path == "test_file1.txt" for f in active_files)
    assert any(f.path == "test_file2.py" for f in active_files)


def test_info(os_instance):
    info = os_instance.info()
    assert "os" in info
    assert "cwd" in info
    assert "shell" in info


def test_add_process(os_instance):
    process = Mock()
    process.pid = 1234
    process.stdout = Mock()
    process.stdout.fileno.return_value = 1
    process_id = os_instance.add_process(process)
    assert process_id == 1234
    assert 1234 in os_instance._processes


def test_get_processes(os_instance):
    process1 = Mock()
    process1.pid = 1234
    process1.args = "python -m http.server 8000"
    process1.stdout = Mock()
    process1.stdout.fileno.return_value = 1
    os_instance.add_process(process1)

    process2 = Mock()
    process2.pid = 5678
    process2.args = "python script.py"
    process2.stdout = Mock()
    process2.stdout.fileno.return_value = 2
    os_instance.add_process(process2)

    processes = os_instance.get_processes()
    assert processes == {1234: "python -m http.server 8000", 5678: "python script.py"}


def test_cancel_process(os_instance):
    process = Mock()
    process.pid = 1234
    process.stdout = Mock()
    process.stdout.fileno.return_value = 1
    os_instance.add_process(process)

    result = os_instance.cancel_process(1234)
    assert result is True
    assert 1234 not in os_instance._processes
    process.terminate.assert_called_once()
