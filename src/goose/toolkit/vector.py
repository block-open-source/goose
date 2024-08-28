import os
import tempfile
import torch
import hashlib
from goose.toolkit.base import Toolkit, tool
from sentence_transformers import SentenceTransformer, util
from goose.cli.session import SessionNotifier
from pathlib import Path


GOOSE_GLOBAL_PATH = Path("~/.config/goose").expanduser()
VECTOR_PATH = GOOSE_GLOBAL_PATH.joinpath("vectors")


class VectorToolkit(Toolkit):
        
    model = SentenceTransformer('all-MiniLM-L6-v2', tokenizer_kwargs={"clean_up_tokenization_spaces": True})

    def get_db_path(self, repo_path):
        # Create a hash of the repo path
        repo_hash = hashlib.md5(repo_path.encode()).hexdigest()
        return VECTOR_PATH.joinpath(f'code_vectors_{repo_hash}.pt')

    def create_vector_db(self, repo_path: str) -> str:
        """
        Create a vector database of the code in the specified directory and store it in a temp file.

        Args:
            repo_path (str): Path to the source code repository.

        Returns:
            str: Path to the created vector database file.
        """
        temp_db_path = self.get_db_path(repo_path)
        VECTOR_PATH.mkdir(parents=True, exist_ok=True)
        self.notifier.status("Scanning repository...")
        file_paths, file_contents = self.scan_repository(repo_path)
        self.notifier.status("Building vector database...")
        embeddings = self.build_vector_database(file_contents)
        self.notifier.status("Saving vector database...")
        self.save_vector_database(file_paths, embeddings, temp_db_path)
        self.notifier.status("Completed vector database creation")
        return temp_db_path

    @tool
    def query_vector_db(self, repo_path: str, query: str) -> str:
        """
        Locate files in a repository that are potentially semantically related to the query and may hint where to look.

        Args:
            repo_path (str): The repository that we will be searching in
            query (str): Query string to search for semantically related files or paths.            
        Returns:
            str: List of semantically relevant files to look in, also consider the paths the files are in.
        """
        temp_db_path = self.get_db_path(repo_path)
        if not os.path.exists(temp_db_path):
            self.notifier.status("Vector database not found. Creating vector database...")
            self.create_vector_db(repo_path)
        self.notifier.status("Loading vector database...")
        file_paths, embeddings = self.load_vector_database(temp_db_path)
        self.notifier.status("Performing query...")
        similar_files = self.find_similar_files(query, file_paths, embeddings)
        self.notifier.status("Query completed")
        return '\n'.join(similar_files)

    def scan_repository(self, repo_path):
        file_contents = []
        file_paths = []
        for root, dirs, files in os.walk(repo_path):
            for file in files:
                if file.endswith(('.py', '.java', '.js', '.cpp', '.c', '.h', '.rb', '.go', '.rs', '.php', '.css', '.md', '.dart', '.kt', '.ts', '.yaml', '.yml')):
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
        data = torch.load(db_path, weights_only=True)
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
        
        return "**When looking at a large repository for relevant files or paths to examine related semantically to the question, use the query_vector_db tool**"


