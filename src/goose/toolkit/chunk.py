from goose.notifier import Notifier
from goose.toolkit.base import Toolkit, tool, Requirements

class FileChunkReader(Toolkit):
    def __init__(self, notifier: Notifier, requires: Requirements):
        super().__init__(notifier, requires)
        self.file_dict = {}

    def system(self):
        return ("If you're reading a file in chunks with the read_chunk tool, read until you find the information you "
                "are looking for, or until you reach the end of the file.")

    @tool
    def read_chunk(self, file_path: str, chunk_size: int) -> str:
        """
        Reads a specified chunk of lines from a file.

        Args:
            file_path (str): The path to the file to be read.
            chunk_size (int): The number of lines to read.

        Returns:
            chunk (str): The chunk of lines read from the file.
        """
        self.notifier.log(f"Reading chunk of {chunk_size} lines from {file_path}")

        # Ensure file data is loaded initially
        if file_path not in self.file_dict:
            with open(file_path, 'r') as file:
                lines = file.readlines()
            self.file_dict[file_path] = {
                "lines": lines,
                "start": 0,
                "stop": 0
            }
        else:
            lines = self.file_dict[file_path]["lines"]

        # Adjust stop index based on chunk size
        self.file_dict[file_path]["stop"] = self.file_dict[file_path]["start"] + chunk_size
        if self.file_dict[file_path]["stop"] > len(lines):
            self.file_dict[file_path]["stop"] = len(lines)

        # Read the chunk
        chunk = '\n'.join(lines[self.file_dict[file_path]["start"]:self.file_dict[file_path]["stop"]])

        # Update start index to the next position
        self.file_dict[file_path]["start"] = self.file_dict[file_path]["stop"]

        # Reset indices if the end of the file is reached
        if self.file_dict[file_path]["start"] >= len(lines):
            self.notifier.log("End of file reached. Popping off file data.")
            self.file_dict.pop(file_path)

        return chunk
