from prompt_toolkit import PromptSession
from prompt_toolkit.application.current import get_app
from prompt_toolkit.key_binding import KeyBindings, KeyPressEvent
from prompt_toolkit.keys import Keys
from prompt_toolkit.styles import Style

from goose.cli.prompt.completer import GoosePromptCompleter
from goose.cli.prompt.lexer import PromptLexer
from goose.command import get_commands


def create_prompt() -> PromptSession:
    # Define custom style
    style = Style.from_dict(
        {
            "parameter": "bold",
            "command": "ansiblue bold",
            "text": "default",
        }
    )

    bindings = KeyBindings()

    # Bind the "Option + Enter" key to insert a newline
    @bindings.add(Keys.Escape, Keys.ControlM)
    def _(event: KeyPressEvent) -> None:
        buffer = event.app.current_buffer
        buffer.insert_text("\n")

    # Bind the "Enter" key to accept the completion if the completion menu is open
    # otherwise just submit the input
    @bindings.add(Keys.Enter)
    def _(event: KeyPressEvent) -> None:
        buffer = event.current_buffer
        app = get_app()

        if app.layout.has_focus(buffer):
            # Check if the completion menu is open
            if buffer.complete_state:
                # accept completion
                buffer.complete_state = None
            else:
                buffer.validate_and_handle()

    @bindings.add(Keys.ControlY)
    def _(event: KeyPressEvent) -> None:
        buffer = event.app.current_buffer
        app = get_app()
        if app.layout.has_focus(buffer):
            # Check if the completion menu is open
            if buffer.complete_state:
                # accept completion
                buffer.complete_state = None

    # instantiate the commands available in the prompt
    commands = dict()
    command_plugins = get_commands()
    for command, command_cls in command_plugins.items():
        commands[command] = command_cls()

    return PromptSession(
        completer=GoosePromptCompleter(commands=commands),
        lexer=PromptLexer(list(commands.keys())),
        style=style,
        key_bindings=bindings,
    )
