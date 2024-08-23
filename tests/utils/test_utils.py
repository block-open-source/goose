import string

import pytest
from goose.utils import droid, ensure, ensure_list, load_plugins


class MockClass:
    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        return self.name == other.name


def test_load_plugins():
    plugins = load_plugins("exchange.provider")
    assert isinstance(plugins, dict)
    assert len(plugins) > 0


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


def test_droid():
    result = droid()
    assert isinstance(result, str)
    assert len(result) == 4
    for character in [result[i] for i in [0, 2]]:
        assert character in string.ascii_lowercase, "should be in lower case"
    for character in [result[i] for i in [1, 3]]:
        assert character in string.digits, "should be a digit"
