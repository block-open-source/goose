import pytest
import time
import requests
from goose.synopsis.toolkit import SynopsisDeveloper
from goose.synopsis.system import system


class MockNotifier:
    def log(self, message):
        pass

    def status(self, message):
        pass


@pytest.fixture
def toolkit(tmpdir):
    original_cwd = system.cwd
    system.cwd = str(tmpdir)
    notifier = MockNotifier()
    toolkit = SynopsisDeveloper(notifier=notifier)

    yield toolkit

    # Teardown: cancel all processes and restore original working directory
    for process_id in list(system._processes.keys()):
        system.cancel_process(process_id)
    system.cwd = original_cwd


def test_start_process(toolkit):
    process_id = toolkit.start_process("python -m http.server 8000")
    assert process_id > 0
    time.sleep(2)  # Give the server time to start

    # Check if the server is running
    try:
        response = requests.get("http://localhost:8000")
        assert response.status_code == 200
    except requests.ConnectionError:
        pytest.fail("HTTP server did not start successfully")
    output = toolkit.view_process_output(process_id)
    assert "200" in output


def test_list_processes(toolkit):
    process_id = toolkit.start_process("python -m http.server 8001")
    processes = toolkit.list_processes()
    assert process_id in processes
    assert "python -m http.server 8001" in processes[process_id]


def test_cancel_process(toolkit):
    process_id = toolkit.start_process("python -m http.server 8003")
    time.sleep(2)  # Give the server time to start

    result = toolkit.cancel_process(process_id)
    assert result == f"process {process_id} cancelled"

    # Verify that the process is no longer running
    with pytest.raises(ValueError):
        toolkit.view_process_output(process_id)
