from pathlib import Path
import os
from tempfile import TemporaryDirectory
from unittest.mock import MagicMock

import pytest
from goose.toolkit.vector import VectorToolkit


@pytest.fixture
def temp_dir():
    with TemporaryDirectory() as temp_dir:
        yield Path(temp_dir)


@pytest.fixture
def vector_toolkit():
    return VectorToolkit(notifier=MagicMock())


def test_query_vector_db_creates_db(temp_dir, vector_toolkit):
    # Create and load a vector database lazily
    query = 'print("Hello World")'
    result = vector_toolkit.find_similar_files_locations(temp_dir.as_posix(), query)
    print("Query Result:", result)
    assert isinstance(result, str)
    temp_db_path = vector_toolkit.get_db_path(temp_dir.as_posix())
    assert os.path.exists(temp_db_path)
    assert os.path.getsize(temp_db_path) > 0


def test_query_vector_db(temp_dir, vector_toolkit):
    # Create initial db
    vector_toolkit.create_vector_db(temp_dir.as_posix())
    query = 'print("Hello World")'
    result = vector_toolkit.find_similar_files_locations(temp_dir.as_posix(), query)
    print("Query Result:", result)
    assert isinstance(result, str)
    temp_db_path = vector_toolkit.get_db_path(temp_dir.as_posix())
    assert os.path.exists(temp_db_path)
    assert os.path.getsize(temp_db_path) > 0
    assert "No embeddings available to query against" in result or "\n" in result


def test_no_new_db_if_exists_higher(temp_dir, vector_toolkit):
    # Create a vector DB at a higher level
    higher_dir = temp_dir / "higher"
    higher_dir.mkdir()
    db_path_higher = vector_toolkit.create_vector_db(higher_dir.as_posix())

    # Now create a lower directory
    lower_dir = higher_dir / "lower"
    lower_dir.mkdir()

    # Perform query on the lower directory
    query = 'print("Hello World")'
    result = vector_toolkit.find_similar_files_locations(lower_dir.as_posix(), query)
    print("Query Result from Lower Directory:", result)

    # Ensure a DB at the lower level is not created
    temp_db_path_lower = vector_toolkit.get_db_path(lower_dir.as_posix())
    assert not os.path.exists(temp_db_path_lower)
    assert os.path.exists(db_path_higher)
    assert os.path.getsize(db_path_higher) > 0


def test_find_similar_files_in_repo(temp_dir, vector_toolkit):
    # Setting up a temporary repository structure
    file_structure = {
        "file1.py": "def function_one(): pass\n",
        "file2.py": "def function_two(): pass\n",
        "subdir": {"file3.py": "class MyClass: pass\n"},
    }

    def create_files(base_path, structure):
        for name, content in structure.items():
            path = base_path / name
            if isinstance(content, str):
                with open(path, "w") as f:
                    f.write(content)
            else:
                path.mkdir()
                create_files(path, content)

    create_files(temp_dir, file_structure)

    # Create initial db
    vector_toolkit.create_vector_db(temp_dir.as_posix())

    # Test query
    query = "def function_one"
    result = vector_toolkit.find_similar_files_locations(temp_dir.as_posix(), query)
    print("Similar Files Result:", result)
    assert "file1.py" in result or "subdir" in result
