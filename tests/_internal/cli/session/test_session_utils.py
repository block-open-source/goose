import string
from goose._internal.cli.session.session_utils import random_session_name

def test_droid():
    result = random_session_name()
    assert isinstance(result, str)
    assert len(result) == 4
    for character in [result[i] for i in [0, 2]]:
        assert character in string.ascii_lowercase, "should be in lower case"
    for character in [result[i] for i in [1, 3]]:
        assert character in string.digits, "should be a digit"
