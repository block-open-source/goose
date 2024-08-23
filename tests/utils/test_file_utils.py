from unittest.mock import patch

import pytest
from goose.utils.file_utils import (
    create_extensions_list,
    create_language_weighting,
)  # Adjust the import path as necessary


# tests for `create_extensions_list`
def test_create_extensions_list_valid_input():
    """Test with valid input and multiple file extensions."""
    project_root = "/fake/project/root"
    max_n = 3
    files = [
        "/fake/project/root/file1.py",
        "/fake/project/root/file2.py",
        "/fake/project/root/file3.md",
        "/fake/project/root/file4.md",
        "/fake/project/root/file5.txt",
        "/fake/project/root/file6.py",
        "/fake/project/root/file7.md",
    ]

    with patch("goose.utils.file_utils.create_file_list", return_value=files):
        extensions = create_extensions_list(project_root, max_n)
        assert extensions == [".py", ".md", ".txt"], "Should return the top 3 extensions in the correct order"


def test_create_extensions_list_zero_max_n():
    """Test that a ValueError is raised when max_n is 0."""
    project_root = "/fake/project/root"
    max_n = 0

    with pytest.raises(ValueError, match="Number of file extensions must be greater than 0"):
        create_extensions_list(project_root, max_n)


def test_create_extensions_list_no_files():
    """Test with a project root that contains no files."""
    project_root = "/fake/project/root"
    max_n = 3

    with patch("goose.utils.file_utils.create_file_list", return_value=[]):
        extensions = create_extensions_list(project_root, max_n)
        assert extensions == [], "Should return an empty list when no files are present"


def test_create_extensions_list_fewer_extensions_than_max_n():
    """Test when there are fewer unique extensions than max_n."""
    project_root = "/fake/project/root"
    max_n = 5
    files = [
        "/fake/project/root/file1.py",
        "/fake/project/root/file2.py",
        "/fake/project/root/file3.md",
    ]

    with patch("goose.utils.file_utils.create_file_list", return_value=files):
        extensions = create_extensions_list(project_root, max_n)
        assert extensions == [".py", ".md"], "Should return all available extensions when fewer than max_n"


def test_create_extensions_list_files_without_extensions():
    """Test that files without extensions are ignored."""
    project_root = "/fake/project/root"
    max_n = 3
    files = [
        "/fake/project/root/file1",
        "/fake/project/root/file2.py",
        "/fake/project/root/file3",
        "/fake/project/root/file4.md",
    ]

    with patch("goose.utils.file_utils.create_file_list", return_value=files):
        extensions = create_extensions_list(project_root, max_n)
        assert extensions == [".py", ".md"], "Should ignore files without extensions"


# tests for `create_language_weighting`
def test_create_language_weighting_normal_case():
    """Test the function with multiple files and different sizes."""
    files = [
        "/fake/project/file1.py",
        "/fake/project/file2.py",
        "/fake/project/file3.md",
        "/fake/project/file4.txt",
    ]

    sizes = {
        "/fake/project/file1.py": 100,
        "/fake/project/file2.py": 200,
        "/fake/project/file3.md": 50,
        "/fake/project/file4.txt": 150,
    }

    # Mocking os.path.getsize to return different sizes for different files
    with patch("os.path.getsize") as mock_getsize:
        mock_getsize.side_effect = lambda file: sizes[file]

        result = create_language_weighting(files)

    total = sum(sizes.values())

    expected_result = {
        ".py": 300 / total * 100,  # 300 out of 600 total
        ".txt": 150 / total * 100,  # 150 out of 600 total
        ".md": 50 / total * 100,  # 50 out of 600 total
    }

    # Check if the result matches the expected output
    assert result[".py"] == pytest.approx(expected_result.get(".py"), 0.01)
    assert result[".txt"] == pytest.approx(expected_result.get(".txt"), 0.01)
    assert result[".md"] == pytest.approx(expected_result.get(".md"), 0.01)


def test_create_language_weighting_no_files():
    """Test the function when no files are provided."""
    files = []

    result = create_language_weighting(files)
    assert result == {}, "Should return an empty dictionary when no files are provided"


def test_create_language_weighting_files_without_extensions():
    """Test the function when files have no extensions."""
    files = [
        "/fake/project/file1",
        "/fake/project/file2",
    ]

    with patch("os.path.getsize", return_value=100):
        result = create_language_weighting(files)

    assert result == {}, "Should return an empty dictionary when files have no extensions"


def test_create_language_weighting_zero_total_size():
    """Test the function when all files have a size of 0."""
    files = [
        "/fake/project/file1.py",
        "/fake/project/file2.py",
    ]

    with patch("os.path.getsize", return_value=0):
        result = create_language_weighting(files)

    assert result == {".py": 0}


def test_create_language_weighting_single_file():
    """Test the function with a single file."""
    files = [
        "/fake/project/file1.py",
    ]

    with patch("os.path.getsize", return_value=100):
        result = create_language_weighting(files)

    assert result == {".py": 100.0}, "Should return 100% for the single file's extension"


def test_create_language_weighting_mixed_extensions():
    """Test the function with files of mixed extensions and sizes."""
    files = [
        "/fake/project/file1.py",
        "/fake/project/file2.py",
        "/fake/project/file3.md",
        "/fake/project/file4.txt",
        "/fake/project/file5.md",
    ]

    with patch("os.path.getsize") as mock_getsize:
        mock_getsize.side_effect = lambda file: {
            "/fake/project/file1.py": 100,
            "/fake/project/file2.py": 100,
            "/fake/project/file3.md": 200,
            "/fake/project/file4.txt": 300,
            "/fake/project/file5.md": 100,
        }[file]

        result = create_language_weighting(files)

    expected_result = {
        ".txt": 37.5,  # 300 out of 800 total
        ".md": 37.5,  # 300 out of 800 total
        ".py": 25.0,  # 200 out of 800 total
    }

    assert result[".txt"] == pytest.approx(expected_result.get(".txt"), 0.01)
    assert result[".md"] == pytest.approx(expected_result.get(".md"), 0.01)
    assert result[".py"] == pytest.approx(expected_result.get(".py"), 0.01)
