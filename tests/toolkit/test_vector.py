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
    result = vector_toolkit.query_vector_db(temp_dir.as_posix(), query)
    print("Query Result:", result)
    assert isinstance(result, str)
    temp_db_path = vector_toolkit.get_db_path(temp_dir.as_posix())
    assert os.path.exists(temp_db_path)
    assert os.path.getsize(temp_db_path) > 0

def test_query_vector_db(temp_dir, vector_toolkit):
    # Create initial db
    vector_toolkit.create_vector_db(temp_dir.as_posix())
    query = 'print("Hello World")'
    result = vector_toolkit.query_vector_db(temp_dir.as_posix(), query)
    print("Query Result:", result)
    assert isinstance(result, str)
    temp_db_path = vector_toolkit.get_db_path(temp_dir.as_posix())
    assert os.path.exists(temp_db_path)
    assert os.path.getsize(temp_db_path) > 0
    assert 'No embeddings available to query against' in result or '\n' in result