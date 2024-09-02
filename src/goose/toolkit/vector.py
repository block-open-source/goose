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
        
    model = SentenceTransformer('multi-qa-MiniLM-L6-cos-v1', tokenizer_kwargs={"clean_up_tokenization_spaces": True})

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
        self.notifier.status("Preparing vector database :: Scanning repository (first time may take a while, please wait...)")
        file_paths, file_contents = self.scan_repository(repo_path)
        self.notifier.status("Preparing vector database :: Building vectors (first time may take a while, please wait...)")
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
        temp_db_path = self.lookup_db_path(repo_path)
        if temp_db_path is None:
            temp_db_path = self.create_vector_db(repo_path)
        self.notifier.status("Loading vector database...")
        file_paths, embeddings = self.load_vector_database(temp_db_path)
        self.notifier.status("Performing query...")
        similar_files = self.find_similar_files(query, file_paths, embeddings)        
        return '\n'.join(similar_files)

    def lookup_db_path(self, repo_path: str) -> str:
        """
        Check if a vector database exists for the given repository path or its parent directories.

        Args:
            repo_path (str): Path to the source code repository.

        Returns:
            str: Path to the existing vector database file, or None if none found.
        """
        current_path = Path(repo_path).expanduser()
        while current_path != current_path.parent:
            temp_db_path = self.get_db_path(str(current_path))
            if os.path.exists(temp_db_path):
                return temp_db_path
            current_path = current_path.parent
        return None

    def scan_repository(self, repo_path):
        repo_path = Path(repo_path).expanduser()
        file_contents = []
        file_paths = []
        skipped_file_types = {}
        for root, dirs, files in os.walk(repo_path):
            # Exclude dotfile directories
            dirs[:] = [d for d in dirs if not d.startswith('.')]
            for file in files:
                file_extension = os.path.splitext(file)[1]
                if file_extension in ['.py', '.java', '.js', '.jsx', '.ts', '.tsx', '.cpp', '.c', '.h', '.hpp', '.rb', '.go', '.rs', '.php', '.md', '.dart', '.kt', '.swift', '.scala', '.lua', '.pl', '.r', '.m', '.mm', '.f', '.jl', '.cs', '.vb', '.pas', '.groovy', '.hs', '.elm', '.erl', '.clj', '.lisp']:
                    file_path = os.path.join(root, file)
                    file_paths.append(file_path)
                    try:
                        with open(file_path, 'r', errors='ignore') as f:
                            content = f.read()
                            file_contents.append(content)
                    except Exception as e:
                        print(f'Error reading {file_path}: {e}')
                else:
                    skipped_file_types[file_extension] = True
        return file_paths, file_contents
    
    def build_vector_database(self, file_contents):
        embeddings = self.model.encode(file_contents, convert_to_tensor=True)
        return embeddings

    def save_vector_database(self, file_paths, embeddings, db_path):
        torch.save({'file_paths': file_paths, 'embeddings': embeddings}, db_path)

    def load_vector_database(self, db_path):
        if db_path is not None and os.path.exists(db_path):
            data = torch.load(db_path, weights_only=True)
        else:
            raise ValueError(f"Database path {db_path} does not exist.")
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
