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

def test_create_vector_db(temp_dir, vector_toolkit):
    # Create some test files
    (temp_dir / 'test1.py').write_text('print("Hello World")')
    (temp_dir / 'test2.py').write_text('def foo():\n    return "bar"')

    print(f"Created test files in: {temp_dir}")
    for path in temp_dir.glob('*'):
        print(f"- {path.name}")
    
    temp_db_path = vector_toolkit.get_db_path(temp_dir.as_posix())

    result = vector_toolkit.create_vector_db(temp_dir.as_posix())
    assert 'Vector database created at' in result
    assert os.path.exists(temp_db_path)
    assert os.path.getsize(temp_db_path) > 0

def test_query_vector_db(temp_dir, vector_toolkit):
    # Create and load a vector database
    vector_toolkit.create_vector_db(temp_dir.as_posix())
    query = 'print("Hello World")'
    result = vector_toolkit.query_vector_db(temp_dir.as_posix(), query)
    print("Query Result:", result)
    assert isinstance(result, str)
    # Ensure no exception and the result is handled gracefully
    assert 'No embeddings available to query against' in result or '\n' in result
