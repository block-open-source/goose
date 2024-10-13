import os
import json
from pygments import highlight
from pygments.lexers import get_lexer_by_name
from pygments.formatters import TerminalFormatter
from goose.toolkit.base import Toolkit, tool
from toolkit.screen import Screen

class SnippetOrganizer(Toolkit):
    
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.screen_tool = Screen()

    @tool
    def save_snippet(self, language: str, title: str, description: str):
        """Prompt user to enter a code snippet and save with metadata."""
        file_path = self.screen_tool.take_screenshot()
        directory = f"snippets/{language}"
        os.makedirs(directory, exist_ok=True)
        
        metadata = {
            'title': title,
            'description': description,
            'file_path': file_path
        }
        
        metadata_path = os.path.join(directory, f"{title}.json")
        with open(metadata_path, 'w') as file:
            json.dump(metadata, file)
        self.notifier.log(f"Snippet '{title}' has been saved under {directory}")

    @tool
    def search_snippets(self, query: str):
        """Search for snippets by title, description, or language."""
        results = []
        if not os.path.exists('snippets'):
            return "No snippets available."
        
        for root, _, files in os.walk('snippets'):
            for file in files:
                if file.endswith('.json'):
                    with open(os.path.join(root, file), 'r') as f:
                        metadata = json.load(f)
                        if (query.lower() in metadata['title'].lower() or
                            query.lower() in metadata['description'].lower() or
                            query.lower() in root.lower()):
                            results.append(metadata)

        if not results:
            return "No matching snippets found."
        
        for result in results:
            self.notifier.log(f"Title: {result['title']}, Description: {result['description']}, File path: {result['file_path']}")

    @tool
    def print_snippet(self, file_path: str, language: str):
        """Display the code snippet with syntax highlighting."""
        try:
            with open(file_path, 'r') as code_file:
                code = code_file.read()
            lexer = get_lexer_by_name(language, stripall=True)
            formatter = TerminalFormatter()
            highlighted_code = highlight(code, lexer, formatter)
            self.notifier.log(highlighted_code)
        except Exception as e:
            self.notifier.log(f"Error printing snippet: {e}")

    def system(self) -> str:
        return "Instructions for using the Snippet Organizer Toolkit"
