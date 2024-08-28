import os
import tempfile
import torch
import uuid
import hashlib
from goose.toolkit.base import Toolkit, tool
from sentence_transformers import SentenceTransformer, util

class VectorToolkit(Toolkit):
    def __init__(self, notifier):
        super().__init__(notifier)
        self.model = SentenceTransformer('all-MiniLM-L6-v2')

    def get_db_path(self, repo_path):
        # Create a hash of the repo path
        repo_hash = hashlib.md5(repo_path.encode()).hexdigest()
        return os.path.join(tempfile.gettempdir(), f'code_vectors_{repo_hash}.pt')

    @tool
    def create_vector_db(self, repo_path: str) -> str:
        """
        Create a vector database of the code in the specified directory and store it in a temp file.

        Args:
            repo_path (str): Path to the source code repository.

        Returns:
            str: Path to the created vector database file.
        """
        temp_db_path = self.get_db_path(repo_path)
        file_paths, file_contents = self.scan_repository(repo_path)
        print("Scanned File Paths:", file_paths)
        embeddings = self.build_vector_database(file_contents)
        self.save_vector_database(file_paths, embeddings, temp_db_path)
        return f'Vector database created at {temp_db_path}'

    @tool
    def query_vector_db(self, repo_path: str, query: str) -> str:
        """
        Query the vector database with the provided string and return similar files.

        Args:
            query (str): Query string to search in the database.

        Returns:
            str: List of similar files found in the vector database.
        """
        temp_db_path = self.get_db_path(repo_path)
        file_paths, embeddings = self.load_vector_database(temp_db_path)
        print("File Paths:", file_paths)
        print("Embeddings Size:", embeddings.size())
        similar_files = self.find_similar_files(query, file_paths, embeddings)
        return '\n'.join(similar_files)

    def scan_repository(self, repo_path):
        print(f'Scanning repository at: {repo_path}')
        file_contents = []
        file_paths = []
        for root, dirs, files in os.walk(repo_path):
            for file in files:
                if file.endswith(('.py', '.java', '.js', '.cpp', '.c', '.h', '.rb', '.go', '.rs', '.php', '.html', '.css', '.md', '.dart')):
                    file_path = os.path.join(root, file)
                    file_paths.append(file_path)
                    try:
                        with open(file_path, 'r', errors='ignore') as f:
                            content = f.read()
                            file_contents.append(content)
                    except Exception as e:
                        print(f'Error reading {file_path}: {e}')
        return file_paths, file_contents

    def build_vector_database(self, file_contents):
        embeddings = self.model.encode(file_contents, convert_to_tensor=True)
        return embeddings

    def save_vector_database(self, file_paths, embeddings, db_path):
        torch.save({'file_paths': file_paths, 'embeddings': embeddings}, db_path)

    def load_vector_database(self, db_path):
        data = torch.load(db_path)
        return data['file_paths'], data['embeddings']

    def find_similar_files(self, query, file_paths, embeddings):
        query_embedding = self.model.encode([query], convert_to_tensor=True)
        if embeddings.size(0) == 0:
            return 'No embeddings available to query against'
        scores = util.pytorch_cos_sim(query_embedding, embeddings)[0]
        top_results = torch.topk(scores, k=10)
        similar_files = [file_paths[idx] for idx in top_results[1]]
        return similar_files

    def system(self) -> str:
        return """**When the user wants to create a vector database or query an existing one, use the create_vector_db and query_vector_db tools respectively.**"""

