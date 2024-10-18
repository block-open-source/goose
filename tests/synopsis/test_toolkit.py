import os
import pytest
from goose.synopsis.toolkit import SynopsisDeveloper
from goose.synopsis.system import system


class MockNotifier:
    def log(self, message):
        pass

    def status(self, message):
        pass


@pytest.fixture
def toolkit(tmpdir):
    original_cwd = os.getcwd()
    os.chdir(tmpdir)
    system.cwd = str(tmpdir)
    notifier = MockNotifier()
    toolkit = SynopsisDeveloper(notifier=notifier)

    yield toolkit

    # Teardown: cancel all processes and restore original working directory
    for process_id in list(system._processes.keys()):
        system.cancel_process(process_id)
    os.chdir(original_cwd)
    system.cwd = original_cwd


def test_shell(toolkit, tmpdir):
    result = toolkit.shell("echo 'Hello, World!'")
    assert "Hello, World!" in result


def test_read_write_file(toolkit, tmpdir):
    test_file = tmpdir.join("test_file.txt")
    content = "Test content"

    toolkit.write_file(str(test_file), content)
    assert test_file.read() == content

    result = toolkit.read_file(str(test_file))
    assert "The file content at" in result
    assert system.is_active(str(test_file))


def test_patch_file(toolkit, tmpdir):
    test_file = tmpdir.join("test_file.txt")
    test_file.write("Hello, World!")

    toolkit.read_file(str(test_file))  # Remember the file
    result = toolkit.patch_file(str(test_file), "World", "Universe")
    assert "Succesfully replaced before with after" in result
    assert test_file.read() == "Hello, Universe!"


def test_change_dir(toolkit, tmpdir):
    subdir = tmpdir.mkdir("subdir")
    result = toolkit.change_dir(str(subdir))
    assert result == str(subdir)
    assert system.cwd == str(subdir)


def test_start_process(toolkit, tmpdir):
    process_id = toolkit.start_process("python -m http.server 8000")
    assert process_id > 0

    # Check if the process is in the list of running processes
    processes = toolkit.list_processes()
    assert process_id in processes
    assert "python -m http.server 8000" in processes[process_id]


def test_list_processes(toolkit, tmpdir):
    process_id1 = toolkit.start_process("python -m http.server 8001")
    process_id2 = toolkit.start_process("python -m http.server 8002")

    processes = toolkit.list_processes()
    assert process_id1 in processes
    assert process_id2 in processes
    assert "python -m http.server 8001" in processes[process_id1]
    assert "python -m http.server 8002" in processes[process_id2]


def test_cancel_process(toolkit, tmpdir):
    process_id = toolkit.start_process("python -m http.server 8003")

    result = toolkit.cancel_process(process_id)
    assert result == f"process {process_id} cancelled"

    # Verify that the process is no longer in the list
    processes = toolkit.list_processes()
    assert process_id not in processes
