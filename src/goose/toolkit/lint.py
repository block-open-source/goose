from goose.utils import load_plugins


def lint_toolkits() -> None:
    for toolkit_name, toolkit in load_plugins("goose.toolkit").items():
        assert toolkit.__doc__ is not None, f"`{toolkit_name}` toolkit must have a docstring"
        first_line_of_docstring = toolkit.__doc__.split("\n")[0]
        assert len(first_line_of_docstring.split(" ")) > 5, f"`{toolkit_name}` toolkit docstring is too short"
        assert len(first_line_of_docstring.split(" ")) < 12, f"`{toolkit_name}` toolkit docstring is too long"
        assert first_line_of_docstring[
            0
        ].isupper(), f"`{toolkit_name}` toolkit docstring must start with a capital letter"
