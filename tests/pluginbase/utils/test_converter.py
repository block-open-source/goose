import pytest
from goose.pluginbase.utils.converter import ensure, ensure_list


class MockClass:
    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        return self.name == other.name

def test_ensure_with_class():
    mock_class = MockClass("foo")
    assert ensure(MockClass)(mock_class) == mock_class


def test_ensure_with_dictionary():
    mock_class = ensure(MockClass)({"name": "foo"})
    assert mock_class == MockClass("foo")


def test_ensure_with_invalid_dictionary():
    with pytest.raises(TypeError):
        ensure(MockClass)({"age": "foo"})


def test_ensure_with_list():
    mock_class = ensure(MockClass)(["foo"])
    assert mock_class == MockClass("foo")


def test_ensure_with_invalid_list():
    with pytest.raises(TypeError):
        ensure(MockClass)(["foo", "bar"])


def test_ensure_with_value():
    mock_class = ensure(MockClass)("foo")
    assert mock_class == MockClass("foo")


def test_ensure_list():
    obj_list = ensure_list(MockClass)(["foo", "bar"])
    assert obj_list == [MockClass("foo"), MockClass("bar")]

