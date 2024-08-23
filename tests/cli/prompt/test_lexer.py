from goose.cli.prompt.lexer import (
    PromptLexer,
    command_itself,
    completion_for_command,
    value_for_command,
)
from prompt_toolkit.document import Document


# Helper function to create a Document and lexer instance
def create_lexer_and_document(commands, text):
    lexer = PromptLexer(commands)
    document = Document(text)
    return lexer, document


# Test cases
def test_lex_document_command():
    lexer, document = create_lexer_and_document(["file"], "/file:example.txt")
    tokens = lexer.lex_document(document)
    expected_tokens = [("class:command", "/file:"), ("class:parameter", "example.txt")]
    assert tokens(0) == expected_tokens


def test_lex_document_partial_command():
    lexer, document = create_lexer_and_document(["file"], "/fi")
    tokens = lexer.lex_document(document)
    expected_tokens = [("class:command", "/fi")]
    assert tokens(0) == expected_tokens


def test_lex_document_with_text():
    lexer, document = create_lexer_and_document(["file"], "Some text /file:example.txt")
    tokens = lexer.lex_document(document)
    expected_tokens = [
        ("class:text", "S"),
        ("class:text", "o"),
        ("class:text", "m"),
        ("class:text", "e"),
        ("class:text", " "),
        ("class:text", "t"),
        ("class:text", "e"),
        ("class:text", "x"),
        ("class:text", "t"),
        ("class:text", " "),
        ("class:command", "/file:"),
        ("class:parameter", "example.txt"),
    ]
    assert tokens(0) == expected_tokens


def test_lex_document_with_command_in_middle():
    lexer, document = create_lexer_and_document(["file"], "Some text /file:example.txt more text")
    tokens = lexer.lex_document(document)
    expected_tokens = [
        ("class:text", "S"),
        ("class:text", "o"),
        ("class:text", "m"),
        ("class:text", "e"),
        ("class:text", " "),
        ("class:text", "t"),
        ("class:text", "e"),
        ("class:text", "x"),
        ("class:text", "t"),
        ("class:text", " "),
        ("class:command", "/file:"),
        ("class:parameter", "example.txt"),
        ("class:text", " "),
        ("class:text", "m"),
        ("class:text", "o"),
        ("class:text", "r"),
        ("class:text", "e"),
        ("class:text", " "),
        ("class:text", "t"),
        ("class:text", "e"),
        ("class:text", "x"),
        ("class:text", "t"),
    ]
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_multiple_commands():
    lexer, document = create_lexer_and_document(
        ["command", "anothercommand"],
        "/command:example1.txt more text /anothercommand:example2.txt",
    )
    tokens = lexer.lex_document(document)
    expected_tokens = [
        ("class:command", "/command:"),
        ("class:parameter", "example1.txt"),
        ("class:text", " "),
        ("class:text", "m"),
        ("class:text", "o"),
        ("class:text", "r"),
        ("class:text", "e"),
        ("class:text", " "),
        ("class:text", "t"),
        ("class:text", "e"),
        ("class:text", "x"),
        ("class:text", "t"),
        ("class:text", " "),
        ("class:command", "/anothercommand:"),
        ("class:parameter", "example2.txt"),
    ]
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_multiple_same_commands():
    lexer, document = create_lexer_and_document(
        ["command"],
        "/command:example1.txt more text /command:example2.txt",
    )
    tokens = lexer.lex_document(document)
    expected_tokens = [
        ("class:command", "/command:"),
        ("class:parameter", "example1.txt"),
        ("class:text", " "),
        ("class:text", "m"),
        ("class:text", "o"),
        ("class:text", "r"),
        ("class:text", "e"),
        ("class:text", " "),
        ("class:text", "t"),
        ("class:text", "e"),
        ("class:text", "x"),
        ("class:text", "t"),
        ("class:text", " "),
        ("class:command", "/command:"),
        ("class:parameter", "example2.txt"),
    ]
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_two_half_commands():
    lexer, document = create_lexer_and_document(
        ["command"],
        "/comma /com",
    )
    tokens = lexer.lex_document(document)
    expected_tokens = [
        ("class:text", "/"),
        ("class:text", "c"),
        ("class:text", "o"),
        ("class:text", "m"),
        ("class:text", "m"),
        ("class:text", "a"),
        ("class:text", " "),
        ("class:command", "/com"),
    ]
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_command_attached_to_pre_string():
    lexer, document = create_lexer_and_document(
        ["command"],
        "some/command:example.txt",
    )
    expected_tokens = [
        ("class:text", "s"),
        ("class:text", "o"),
        ("class:text", "m"),
        ("class:text", "e"),
        ("class:text", "/"),
        ("class:text", "c"),
        ("class:text", "o"),
        ("class:text", "m"),
        ("class:text", "m"),
        ("class:text", "a"),
        ("class:text", "n"),
        ("class:text", "d"),
        ("class:text", ":"),
        ("class:text", "e"),
        ("class:text", "x"),
        ("class:text", "a"),
        ("class:text", "m"),
        ("class:text", "p"),
        ("class:text", "l"),
        ("class:text", "e"),
        ("class:text", "."),
        ("class:text", "t"),
        ("class:text", "x"),
        ("class:text", "t"),
    ]
    tokens = lexer.lex_document(document)
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_partial_command_attached_to_pre_string():
    lexer, document = create_lexer_and_document(
        ["command"],
        "some/com",
    )
    tokens = lexer.lex_document(document)
    expected_tokens = [
        ("class:text", "s"),
        ("class:text", "o"),
        ("class:text", "m"),
        ("class:text", "e"),
        ("class:text", "/"),
        ("class:text", "c"),
        ("class:text", "o"),
        ("class:text", "m"),
    ]
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_no_command():
    lexer, document = create_lexer_and_document([], "Some random text")
    tokens = lexer.lex_document(document)
    expected_tokens = [("class:text", character) for character in "Some random text"]
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_lex_document_ending_char_of_parameter_is_symbol():
    lexer, document = create_lexer_and_document(
        ["command"],
        "/command:example.txt/",
    )
    expected_tokens = [
        ("class:command", "/command:"),
        ("class:parameter", "example.txt/"),
    ]
    tokens = lexer.lex_document(document)
    actual_tokens = list(tokens(0))
    assert actual_tokens == expected_tokens


def test_command_itself():
    pattern = command_itself("file:")
    matches = pattern.match("/file:example.txt")
    assert matches is not None
    assert matches.group(1) == "/file:"


def test_value_for_command():
    pattern = value_for_command("file:")
    matches = pattern.search("/file:example.txt")
    assert matches is not None
    assert matches.group(1) == "example.txt"


def test_completion_for_command():
    pattern = completion_for_command("file:")
    matches = pattern.search("/file:")
    assert matches is not None
    assert matches.group(1) == "file:"
