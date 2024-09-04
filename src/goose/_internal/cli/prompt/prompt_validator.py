from prompt_toolkit.document import Document
from prompt_toolkit.validation import ValidationError, Validator


class PromptValidator(Validator):
    def validate(self, document: Document) -> None:
        text = document.text
        if text is not None and not text.strip():
            message = "Enter your prompt to goose. If you would like to exit, use CTRL+D, or type 'exit' or ':q'"
            raise ValidationError(message=message, cursor_position=0)
