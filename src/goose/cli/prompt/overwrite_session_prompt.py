from typing import Any

from rich.prompt import Prompt


class OverwriteSessionPrompt(Prompt):
    def __init__(self, *args: tuple[Any], **kwargs: dict[str, Any]) -> None:
        super().__init__(*args, **kwargs)
        self.choices = {
            "yes": "Overwrite the existing session",
            "no": "Pick a new session name",
            "resume": "Resume the existing session",
        }
        self.default = "resume"

    def check_choice(self, choice: str) -> bool:
        for key in self.choices:
            normalized_choice = choice.lower()
            if normalized_choice == key or normalized_choice[0] == key[0]:
                return True
        return False

    def pre_prompt(self) -> str:
        print("Would you like to overwrite it?")
        print()
        for key, value in self.choices.items():
            first_letter, remaining = key[0], key[1:]
            rendered_key = rf"[{first_letter}]{remaining}"
            print(f"  {rendered_key:10} {value}")
        print()
