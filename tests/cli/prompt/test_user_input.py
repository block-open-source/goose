from goose.cli.prompt.user_input import PromptAction, UserInput


def test_user_input_with_action_continue():
    input = UserInput(action=PromptAction.CONTINUE, text="Hello")
    assert input.to_continue() is True
    assert input.to_exit() is False
    assert input.text == "Hello"


def test_user_input_with_action_exit():
    input = UserInput(action=PromptAction.EXIT)
    assert input.to_continue() is False
    assert input.to_exit() is True
    assert input.text is None
